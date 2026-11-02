//! Bounded, owner-thread-only High-profile H.264 encoding with VideoToolbox.
//!
//! Input is explicitly BT.709 limited-range I420. The native adapter copies it
//! into framework-owned storage; no caller borrow survives submission.

mod sys;

use std::collections::VecDeque;
use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, Weak,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Instant;

use crate::h264::{self, AccessUnit, Config, Dimensions, Sync};

/// A checked platform operation used in diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Create a compression session.
    Create,
    /// Set a required property.
    Configure,
    /// Query a property.
    Query,
    /// Prepare the encoder.
    Prepare,
    /// Allocate a bounded pixel buffer.
    PixelBuffer,
    /// Lock pixel storage.
    Lock,
    /// Unlock pixel storage.
    Unlock,
    /// Submit a picture.
    Encode,
    /// Complete pending pictures.
    Complete,
    /// Copy compressed storage.
    BlockCopy,
    /// Receive encoded output.
    Callback,
    /// Inspect a sample description.
    Format,
}

/// Documented pre-submission availability failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// No matching encoder exists.
    NoEncoder,
    /// A matching encoder is temporarily unavailable.
    TemporarilyUnavailable,
}

/// A request or operation that could not complete safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Invalid source data, settings or time.
    Input,
    /// An operation is invalid in the session's current phase.
    State,
    /// A resource limit was reached.
    Limit,
    /// Bounded owned allocation failed.
    Allocation,
    /// Cancellation was requested.
    Cancelled,
    /// The single session deadline expired.
    Deadline,
    /// Output or final accounting is invalid.
    Output,
    /// The required implementation kind could not be established.
    UnknownHardware,
    /// Pull output before submitting another picture; the session is not failed.
    Backpressure,
    /// H.264 metadata validation failed.
    Metadata(h264::Error),
    /// A matching encoder is unavailable before submission.
    Unavailable(Availability),
    /// A native operation returned an error.
    Framework {
        /// The closed operation label.
        operation: Operation,
        /// Apple's signed status value.
        status: i32,
    },
    /// A required control did not retain its requested value.
    Setting(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(error) => error.fmt(f),
            Self::Framework { operation, status } => {
                write!(f, "VideoToolbox {operation:?} failed with status {status}")
            }
            Self::Setting(name) => write!(f, "VideoToolbox did not retain {name}"),
            _ => f.write_str(match self {
                Self::Input => "invalid VideoToolbox input or settings",
                Self::State => "invalid VideoToolbox session state",
                Self::Limit => "VideoToolbox resource limit exceeded",
                Self::Allocation => "cannot reserve VideoToolbox output storage",
                Self::Cancelled => "VideoToolbox encoding cancelled",
                Self::Deadline => "VideoToolbox deadline expired",
                Self::Output => "invalid VideoToolbox output or accounting",
                Self::UnknownHardware => "cannot establish VideoToolbox hardware status",
                Self::Backpressure => "pull VideoToolbox output before submitting more frames",
                Self::Unavailable(_) => "the requested VideoToolbox encoder is unavailable",
                _ => "VideoToolbox encoding failed",
            }),
        }
    }
}

impl std::error::Error for Error {}
impl From<h264::Error> for Error {
    fn from(value: h264::Error) -> Self {
        Self::Metadata(value)
    }
}

/// Actual implementation status, not the requested preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hardware {
    /// The session reports hardware acceleration.
    Hardware,
    /// The session reports no hardware acceleration.
    Software,
    /// The session did not provide a usable status.
    Unknown,
}

/// Selection policy checked against the instantiated session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwarePolicy {
    /// Permit either implementation, retaining unknown status as unknown.
    Any,
    /// Require a confirmed hardware implementation.
    Require,
    /// Disable hardware and require a confirmed software implementation.
    Disable,
}

/// Metadata queried from the actual session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Bounded platform identifier, or absent when the API does not expose one.
    pub encoder_id: Option<String>,
    /// Actual hardware-use status.
    pub hardware: Hardware,
}

/// Exact nonnegative time in a positive media timescale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Time {
    value: i64,
    scale: i32,
}

impl Time {
    /// Checks a rational time without conversion through seconds.
    pub fn new(value: i64, scale: i32) -> Result<Self, Error> {
        if value < 0 || scale <= 0 {
            return Err(Error::Input);
        }
        Ok(Self { value, scale })
    }
    /// The integer numerator.
    pub fn value(self) -> i64 {
        self.value
    }
    /// The positive denominator.
    pub fn scale(self) -> i32 {
        self.scale
    }
    fn compare(self, other: Self) -> std::cmp::Ordering {
        (i128::from(self.value) * i128::from(other.scale))
            .cmp(&(i128::from(other.value) * i128::from(self.scale)))
    }
    fn same(self, other: Self) -> bool {
        self.compare(other).is_eq()
    }
}

/// High-profile, progressive BT.709 limited-range 8-bit 4:2:0 settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    dimensions: Dimensions,
    rate_num: u32,
    rate_den: u32,
    bitrate: u32,
    hardware: HardwarePolicy,
}

impl Settings {
    /// Checks the explicit source format, bitrate and nominal frame clock.
    pub fn bt709_limited(
        width: u32,
        height: u32,
        rate_num: u32,
        rate_den: u32,
        bitrate: u32,
        hardware: HardwarePolicy,
    ) -> Result<Self, Error> {
        let dimensions = Dimensions::new(width, height).map_err(|_| Error::Input)?;
        if width > i32::MAX as u32
            || height > i32::MAX as u32
            || rate_num == 0
            || rate_den == 0
            || rate_num > i32::MAX as u32
            || rate_den > i32::MAX as u32
            || u64::from(rate_num) < u64::from(rate_den)
            || u64::from(rate_num) > 240 * u64::from(rate_den)
            || bitrate == 0
            || bitrate > i32::MAX as u32
        {
            return Err(Error::Input);
        }
        Ok(Self {
            dimensions,
            rate_num,
            rate_den,
            bitrate,
            hardware,
        })
    }
    /// Visible source dimensions.
    pub fn dimensions(self) -> Dimensions {
        self.dimensions
    }
    /// Nominal frames-per-second numerator and denominator.
    pub fn rate(self) -> (u32, u32) {
        (self.rate_num, self.rate_den)
    }
    /// Whole-frame cadence hint, rounded to nearest with ties upward.
    ///
    /// This optimization hint does not replace the exact source clock in `rate`.
    pub fn expected_frame_rate_hint(self) -> u32 {
        let whole = self.rate_num / self.rate_den;
        let remainder = self.rate_num % self.rate_den;
        whole + u32::from(2 * u64::from(remainder) >= u64::from(self.rate_den))
    }
    /// Required average bitrate, not a hard output-byte limit.
    pub fn bitrate(self) -> u32 {
        self.bitrate
    }
    /// Requested implementation policy.
    pub fn hardware_policy(self) -> HardwarePolicy {
        self.hardware
    }
}

/// Borrowed BT.709 limited-range planar I420, copied during submission.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    dimensions: Dimensions,
    y: &'a [u8],
    u: &'a [u8],
    v: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Checks exact tightly packed Y, U and V plane extents.
    pub fn bt709_limited(data: &'a [u8], width: u32, height: u32) -> Result<Self, Error> {
        let dimensions = Dimensions::new(width, height).map_err(|_| Error::Input)?;
        let pixels =
            usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| Error::Limit)?;
        let end_u = pixels.checked_add(pixels / 4).ok_or(Error::Limit)?;
        let end_v = end_u.checked_add(pixels / 4).ok_or(Error::Limit)?;
        if data.len() != end_v {
            return Err(Error::Input);
        }
        Ok(Self {
            dimensions,
            y: data.get(..pixels).ok_or(Error::Input)?,
            u: data.get(pixels..end_u).ok_or(Error::Input)?,
            v: data.get(end_u..end_v).ok_or(Error::Input)?,
        })
    }
}

/// Per-session limits; framework-internal allocations are not a process budget.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum coded width.
    pub max_width: u32,
    /// Maximum coded height.
    pub max_height: u32,
    /// Maximum accepted source-frame count.
    pub max_frames: u64,
    /// Maximum pending or queued source identities together.
    pub max_pending_frames: usize,
    /// Maximum bytes in one owned access unit.
    pub max_access_unit_bytes: usize,
    /// Maximum complete SPS/PPS/avcC size.
    pub max_configuration_bytes: usize,
    /// Maximum NAL units per access unit.
    pub max_nals: usize,
    /// Maximum cumulative encoded bytes.
    pub max_output_bytes: u64,
    /// Maximum reserved access-unit bytes in the pending ledger.
    pub max_queue_bytes: usize,
    /// Maximum checked storage in one framework pixel buffer.
    pub max_pixel_buffer_bytes: usize,
    /// Threshold for buffers allocated through the native pool.
    pub max_pool_buffers: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_width: 4096,
            max_height: 2304,
            max_frames: 10_000_000,
            max_pending_frames: 4,
            max_access_unit_bytes: 4 * 1024 * 1024,
            max_configuration_bytes: 16 * 1024,
            max_nals: 4096,
            max_output_bytes: 1024 * 1024 * 1024,
            max_queue_bytes: 16 * 1024 * 1024,
            max_pixel_buffer_bytes: 32 * 1024 * 1024,
            max_pool_buffers: 8,
        }
    }
}

impl Limits {
    fn reader(self) -> h264::Limits {
        h264::Limits {
            max_configuration_bytes: self.max_configuration_bytes,
            max_width: self.max_width,
            max_height: self.max_height,
            max_access_unit_bytes: self.max_access_unit_bytes,
            max_nals: self.max_nals,
        }
    }
    fn reservation(self, settings: Settings) -> Result<usize, Error> {
        if self.max_width < settings.dimensions.width()
            || self.max_height < settings.dimensions.height()
            || self.max_frames == 0
            || self.max_frames >= isize::MAX as u64
            || self.max_pending_frames == 0
            || self.max_pending_frames > 64
            || self.max_access_unit_bytes == 0
            || self.max_access_unit_bytes > isize::MAX as usize
            || self.max_configuration_bytes == 0
            || self.max_configuration_bytes > isize::MAX as usize
            || self.max_nals == 0
            || self.max_queue_bytes < self.max_access_unit_bytes
            || self.max_pool_buffers == 0
            || self.max_pixel_buffer_bytes == 0
        {
            return Err(Error::Limit);
        }
        self.max_access_unit_bytes
            .checked_mul(4)
            .and_then(|n| {
                self.max_configuration_bytes
                    .checked_mul(4)
                    .and_then(|config| n.checked_add(config))
            })
            .and_then(|n| n.checked_add(std::mem::size_of::<Slot>()))
            .and_then(|n| n.checked_mul(self.max_pending_frames))
            .and_then(|n| n.checked_add(std::mem::size_of::<Mailbox>()))
            .ok_or(Error::Limit)
    }
}

struct Signal {
    cancelled: AtomicBool,
    listeners: Mutex<Vec<Weak<Mailbox>>>,
}

/// Shared cancellation and one monotonic deadline for a session.
#[derive(Clone)]
pub struct Control {
    signal: Arc<Signal>,
    deadline: Instant,
}

impl Control {
    /// Creates a control without extending its deadline on subsequent calls.
    pub fn new(deadline: Instant) -> Self {
        Self {
            signal: Arc::new(Signal {
                cancelled: AtomicBool::new(false),
                listeners: Mutex::new(Vec::new()),
            }),
            deadline,
        }
    }
    /// Cancels Rust work and wakes waits, without calling the native session.
    pub fn cancel(&self) {
        self.signal.cancelled.store(true, Ordering::Release);
        let listeners = lock(&self.signal.listeners);
        for weak in listeners.iter() {
            if let Some(mailbox) = weak.upgrade() {
                mailbox.fail(Error::Cancelled);
            }
        }
    }
    fn check(&self) -> Result<(), Error> {
        if self.signal.cancelled.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Deadline);
        }
        Ok(())
    }
    fn listen(&self, mailbox: &Arc<Mailbox>) -> Result<(), Error> {
        let mut listeners = lock(&self.signal.listeners);
        listeners.retain(|weak| weak.strong_count() != 0);
        listeners.try_reserve(1).map_err(|_| Error::Allocation)?;
        listeners.push(Arc::downgrade(mailbox));
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

const MAX_MAILBOXES: usize = 64;
const MAX_RESERVED_BYTES: usize = 256 * 1024 * 1024;
static LIVE_MAILBOXES: AtomicUsize = AtomicUsize::new(0);
static RESERVED_BYTES: AtomicUsize = AtomicUsize::new(0);
static NEXT_KEY: AtomicUsize = AtomicUsize::new(1);
static REGISTRY: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

struct Permit {
    bytes: usize,
}

fn reserve_counter(counter: &AtomicUsize, amount: usize, limit: usize) -> Result<usize, Error> {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let next = current
            .checked_add(amount)
            .filter(|&n| n <= limit)
            .ok_or(Error::Limit)?;
        match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(previous) => return Ok(previous),
            Err(actual) => current = actual,
        }
    }
}

impl Permit {
    fn reserve(bytes: usize) -> Result<Self, Error> {
        reserve_counter(&LIVE_MAILBOXES, 1, MAX_MAILBOXES)?;
        if reserve_counter(&RESERVED_BYTES, bytes, MAX_RESERVED_BYTES).is_err() {
            LIVE_MAILBOXES.fetch_sub(1, Ordering::AcqRel);
            return Err(Error::Limit);
        }
        Ok(Self { bytes })
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        RESERVED_BYTES.fetch_sub(self.bytes, Ordering::AcqRel);
        LIVE_MAILBOXES.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Entry {
    key: usize,
    mailbox: Arc<Mailbox>,
}

struct Registration {
    key: usize,
    active: bool,
}

impl Registration {
    fn new(mailbox: Arc<Mailbox>) -> Result<Self, Error> {
        let key = reserve_counter(&NEXT_KEY, 1, isize::MAX as usize)?;
        let mut registry = lock(&REGISTRY);
        registry.try_reserve(1).map_err(|_| Error::Allocation)?;
        registry.push(Entry { key, mailbox });
        Ok(Self { key, active: true })
    }
    fn retire(&mut self) {
        if !self.active {
            return;
        }
        retire_callback(self.key);
        self.active = false;
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.retire();
    }
}

fn lookup(key: usize) -> Option<Arc<Mailbox>> {
    let registry = lock(&REGISTRY);
    registry
        .iter()
        .find(|entry| entry.key == key)
        .map(|entry| Arc::clone(&entry.mailbox))
}

fn retire_callback(key: usize) {
    if let Some(mailbox) = lookup(key) {
        mailbox.close();
    }
    let mut registry = lock(&REGISTRY);
    if let Some(index) = registry.iter().position(|entry| entry.key == key) {
        registry.swap_remove(index);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Draining,
    Drained,
    Closed,
}

enum SlotState {
    Pending,
    Copying,
    Ready(Packet),
}

struct Slot {
    id: u64,
    pts: Time,
    duration: Time,
    committed: bool,
    state: SlotState,
}

struct Inner {
    phase: Phase,
    error: Option<Error>,
    slots: VecDeque<Slot>,
    config: Option<Arc<Config>>,
    accepted: u64,
    completed: u64,
    delivered: u64,
    total_bytes: u64,
    unclaimed_bytes: u64,
    queue_reserved: usize,
}

struct Mailbox {
    inner: Mutex<Inner>,
    changed: Condvar,
    settings: Settings,
    limits: Limits,
    control: Control,
    _permit: Permit,
}

impl Mailbox {
    fn new(settings: Settings, limits: Limits, control: Control) -> Result<Arc<Self>, Error> {
        let permit = Permit::reserve(limits.reservation(settings)?)?;
        let mut slots = VecDeque::new();
        slots
            .try_reserve_exact(limits.max_pending_frames)
            .map_err(|_| Error::Allocation)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Inner {
                phase: Phase::Running,
                error: None,
                slots,
                config: None,
                accepted: 0,
                completed: 0,
                delivered: 0,
                total_bytes: 0,
                unclaimed_bytes: 0,
                queue_reserved: 0,
            }),
            changed: Condvar::new(),
            settings,
            limits,
            control,
            _permit: permit,
        }))
    }
    fn check(&self, inner: &Inner) -> Result<(), Error> {
        if let Some(error) = inner.error {
            return Err(error);
        }
        self.control.check()
    }
    fn fail(&self, error: Error) {
        let mut inner = lock(&self.inner);
        if inner.phase != Phase::Closed && inner.error.is_none() {
            inner.error = Some(error);
        }
        self.changed.notify_all();
    }
    fn close(&self) {
        let mut inner = lock(&self.inner);
        inner.phase = Phase::Closed;
        inner.slots.clear();
        inner.config = None;
        self.changed.notify_all();
    }
}

struct RawSample {
    bytes: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    dimensions: (u32, u32),
    pts: Time,
    duration: Time,
    dts: Option<Time>,
    sync: bool,
}

struct CallbackLease {
    mailbox: Arc<Mailbox>,
    id: u64,
    finished: bool,
}

fn lease_callback(key: usize, frame_key: usize) -> Option<CallbackLease> {
    let mailbox = lookup(key)?;
    let id = frame_key.checked_sub(1).map(|n| n as u64);
    {
        let mut inner = lock(&mailbox.inner);
        if inner.phase == Phase::Closed || mailbox.check(&inner).is_err() {
            return None;
        }
        let slot = id.and_then(|id| inner.slots.iter_mut().find(|slot| slot.id == id));
        match slot {
            Some(slot) if matches!(slot.state, SlotState::Pending) => {
                slot.state = SlotState::Copying;
            }
            _ => {
                inner.error = Some(Error::Output);
                mailbox.changed.notify_all();
                return None;
            }
        }
    }
    Some(CallbackLease {
        mailbox,
        id: id?,
        finished: false,
    })
}

fn callback_failed(key: usize) {
    if let Some(mailbox) = lookup(key) {
        mailbox.fail(Error::Output);
    }
}

impl CallbackLease {
    fn control(&self) -> Control {
        self.mailbox.control.clone()
    }
    fn settings(&self) -> Settings {
        self.mailbox.settings
    }
    fn limits(&self) -> Limits {
        self.mailbox.limits
    }
    fn finish(mut self, raw: Result<RawSample, Error>) {
        {
            let mut inner = lock(&self.mailbox.inner);
            if inner.phase != Phase::Closed && inner.error.is_none() {
                let result = self.commit(&mut inner, raw);
                if let Err(error) = result {
                    inner.error = Some(error);
                }
            }
            self.mailbox.changed.notify_all();
        }
        self.finished = true;
    }
    fn commit(&self, inner: &mut Inner, raw: Result<RawSample, Error>) -> Result<(), Error> {
        self.mailbox.check(inner)?;
        let raw = raw?;
        let limits = self.mailbox.limits;
        let dimensions = self.mailbox.settings.dimensions;
        if raw.dimensions != (dimensions.width(), dimensions.height())
            || raw.bytes.len() > limits.max_access_unit_bytes
        {
            return Err(Error::Output);
        }
        let slot = inner
            .slots
            .iter()
            .find(|slot| slot.id == self.id)
            .ok_or(Error::Output)?;
        if !matches!(slot.state, SlotState::Copying)
            || !raw.pts.same(slot.pts)
            || !raw.duration.same(slot.duration)
            || raw.dts.is_some_and(|time| !time.same(slot.pts))
        {
            return Err(Error::Output);
        }
        if let Some(config) = inner.config.as_ref() {
            if config.sps() != raw.sps || config.pps() != raw.pps {
                return Err(Error::Output);
            }
        } else {
            let config =
                Config::from_parameter_sets(&raw.sps, &raw.pps, dimensions, limits.reader())?;
            if config.color() != Some(h264::Color::BT709_LIMITED) {
                return Err(Error::Output);
            }
            inner.config = Some(Arc::new(config));
        }
        let config = inner.config.as_ref().ok_or(Error::Output)?;
        let unit = AccessUnit::copy_from(
            &raw.bytes,
            config,
            if raw.sync { Sync::Idr } else { Sync::NonIdr },
            limits.reader(),
        )?;
        let bytes = u64::try_from(unit.as_bytes().len()).map_err(|_| Error::Limit)?;
        let total = inner.total_bytes.checked_add(bytes).ok_or(Error::Limit)?;
        if total > limits.max_output_bytes || (self.id == 0 && unit.sync() != Sync::Idr) {
            return Err(Error::Output);
        }
        let slot = inner
            .slots
            .iter_mut()
            .find(|slot| slot.id == self.id)
            .ok_or(Error::Output)?;
        slot.state = SlotState::Ready(Packet {
            frame_id: self.id,
            pts: slot.pts,
            duration: slot.duration,
            config: Arc::clone(config),
            unit,
        });
        inner.total_bytes = total;
        inner.unclaimed_bytes = inner
            .unclaimed_bytes
            .checked_sub(limits.max_access_unit_bytes as u64)
            .ok_or(Error::Output)?;
        inner.completed = inner.completed.checked_add(1).ok_or(Error::Limit)?;
        Ok(())
    }
}

impl Drop for CallbackLease {
    fn drop(&mut self) {
        if !self.finished {
            self.mailbox.fail(Error::Output);
        }
    }
}

/// One complete owned access unit and its checked source identity and timing.
#[derive(Debug)]
pub struct Packet {
    /// Sequential source identity.
    pub frame_id: u64,
    /// Exact submitted presentation time.
    pub pts: Time,
    /// Exact submitted duration.
    pub duration: Time,
    /// Shared immutable parameter sets for streaming elementary output.
    pub config: Arc<Config>,
    /// Checked owned H.264 data.
    pub unit: AccessUnit,
}

/// Checked completion after all submitted output has been pulled.
#[derive(Debug)]
pub struct Completion {
    /// Final checked parameter sets and avcC.
    pub config: Arc<Config>,
    /// Actual session implementation.
    pub identity: Identity,
    /// Effective requested-and-checked settings.
    pub settings: Settings,
    /// Number of accepted and delivered source pictures.
    pub frames: u64,
    /// Total delivered access-unit bytes.
    pub bytes: u64,
}

trait Backend {
    fn identity(&self) -> Result<Identity, Error>;
    fn submit(
        &mut self,
        frame: Frame<'_>,
        key: usize,
        pts: Time,
        duration: Time,
    ) -> Result<(), Error>;
    fn complete(&mut self, through: Option<Time>) -> Result<(), Error>;
    fn close(&mut self);
}

impl Backend for sys::Native {
    fn identity(&self) -> Result<Identity, Error> {
        sys::Native::identity(self)
    }
    fn submit(
        &mut self,
        frame: Frame<'_>,
        key: usize,
        pts: Time,
        duration: Time,
    ) -> Result<(), Error> {
        sys::Native::submit(self, frame, key, pts, duration)
    }
    fn complete(&mut self, through: Option<Time>) -> Result<(), Error> {
        sys::Native::complete(self, through)
    }
    fn close(&mut self) {
        sys::Native::close(self);
    }
}

struct Session<B: Backend> {
    native: Option<B>,
    registration: Registration,
    mailbox: Arc<Mailbox>,
    settings: Settings,
    control: Control,
    last_pts: Option<Time>,
    last_end: Option<(Time, Time)>,
}

impl<B: Backend> Session<B> {
    /// Creates, prepares and checks one session before any source is accepted.
    fn new(
        settings: Settings,
        limits: Limits,
        control: Control,
        create: impl FnOnce(usize) -> Result<B, Error>,
    ) -> Result<Self, Error> {
        control.check()?;
        let mailbox = Mailbox::new(settings, limits, control.clone())?;
        control.listen(&mailbox)?;
        let registration = Registration::new(Arc::clone(&mailbox))?;
        let mut owner = Self {
            native: None,
            registration,
            mailbox,
            settings,
            control,
            last_pts: None,
            last_end: None,
        };
        owner.check()?;
        owner.native = Some(create(owner.registration.key)?);
        owner.check()?;
        owner.identity()?;
        Ok(owner)
    }
    fn check(&self) -> Result<(), Error> {
        let inner = lock(&self.mailbox.inner);
        self.mailbox.check(&inner)
    }
    fn record<T>(&self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(error) = result {
            if error != Error::Backpressure {
                self.mailbox.fail(error);
            }
        }
        result
    }
    /// Queries actual identity, without inferring hardware from a preference.
    pub fn identity(&self) -> Result<Identity, Error> {
        let result = self.identity_inner();
        self.record(result)
    }
    fn identity_inner(&self) -> Result<Identity, Error> {
        self.check()?;
        let identity = self.native.as_ref().ok_or(Error::State)?.identity()?;
        match (self.settings.hardware, identity.hardware) {
            (HardwarePolicy::Require, Hardware::Hardware)
            | (HardwarePolicy::Disable, Hardware::Software)
            | (HardwarePolicy::Any, _) => Ok(identity),
            _ => Err(Error::UnknownHardware),
        }
    }
    /// Copies and submits one picture. Backpressure requires pulling, not retrying blindly.
    pub fn submit(&mut self, frame: Frame<'_>, pts: Time, duration: Time) -> Result<(), Error> {
        let result = self.submit_inner(frame, pts, duration);
        self.record(result)
    }
    fn submit_inner(&mut self, frame: Frame<'_>, pts: Time, duration: Time) -> Result<(), Error> {
        self.check()?;
        if frame.dimensions != self.settings.dimensions
            || duration.value == 0
            || self.last_pts.is_some_and(|last| !pts.compare(last).is_gt())
            || self.last_end.is_some_and(|(last, length)| {
                let left =
                    i128::from(pts.value) * i128::from(last.scale) * i128::from(length.scale);
                let right = (i128::from(last.value) * i128::from(length.scale)
                    + i128::from(length.value) * i128::from(last.scale))
                    * i128::from(pts.scale);
                left < right
            })
        {
            return Err(Error::Input);
        }
        let id;
        {
            let mut inner = lock(&self.mailbox.inner);
            self.mailbox.check(&inner)?;
            if inner.phase != Phase::Running {
                return Err(Error::State);
            }
            id = inner.accepted;
            if id >= self.mailbox.limits.max_frames {
                return Err(Error::Limit);
            }
            let limits = self.mailbox.limits;
            let queue = inner
                .queue_reserved
                .checked_add(limits.max_access_unit_bytes)
                .ok_or(Error::Limit)?;
            if inner.slots.len() >= limits.max_pending_frames || queue > limits.max_queue_bytes {
                return Err(Error::Backpressure);
            }
            let unclaimed = inner
                .unclaimed_bytes
                .checked_add(limits.max_access_unit_bytes as u64)
                .ok_or(Error::Limit)?;
            if inner
                .total_bytes
                .checked_add(unclaimed)
                .is_none_or(|n| n > limits.max_output_bytes)
            {
                return Err(Error::Limit);
            }
            inner.slots.push_back(Slot {
                id,
                pts,
                duration,
                committed: false,
                state: SlotState::Pending,
            });
            inner.queue_reserved = queue;
            inner.unclaimed_bytes = unclaimed;
        }
        let key = usize::try_from(id)
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::Limit)?;
        self.native
            .as_mut()
            .ok_or(Error::State)?
            .submit(frame, key, pts, duration)
            .map_err(|error| match error {
                Error::Backpressure => Error::Limit,
                other => other,
            })?;
        {
            let mut inner = lock(&self.mailbox.inner);
            self.mailbox.check(&inner)?;
            let slot = inner
                .slots
                .iter_mut()
                .find(|slot| slot.id == id)
                .ok_or(Error::Output)?;
            slot.committed = true;
            inner.accepted = inner.accepted.checked_add(1).ok_or(Error::Limit)?;
        }
        self.last_pts = Some(pts);
        self.last_end = Some((pts, duration));
        self.check()
    }
    /// Completes the oldest pending picture when necessary, then returns ordered output.
    pub fn pull(&mut self) -> Result<Option<Packet>, Error> {
        let result = self.pull_inner();
        self.record(result)
    }
    fn pull_inner(&mut self) -> Result<Option<Packet>, Error> {
        self.check()?;
        let through = {
            let inner = lock(&self.mailbox.inner);
            match inner.slots.front() {
                Some(slot) if !matches!(slot.state, SlotState::Ready(_)) => Some(slot.pts),
                _ => None,
            }
        };
        if let Some(time) = through {
            self.native
                .as_mut()
                .ok_or(Error::State)?
                .complete(Some(time))?;
            self.control.check()?;
        }
        let mut inner = lock(&self.mailbox.inner);
        loop {
            self.mailbox.check(&inner)?;
            if inner.phase == Phase::Closed {
                return Err(Error::State);
            }
            if inner.slots.is_empty() {
                if inner.phase == Phase::Draining {
                    inner.phase = Phase::Drained;
                }
                return Ok(None);
            }
            if inner
                .slots
                .front()
                .is_some_and(|slot| slot.committed && matches!(slot.state, SlotState::Ready(_)))
            {
                let slot = inner.slots.pop_front().ok_or(Error::Output)?;
                let SlotState::Ready(packet) = slot.state else {
                    return Err(Error::Output);
                };
                if packet.frame_id != inner.delivered {
                    return Err(Error::Output);
                }
                inner.delivered = inner.delivered.checked_add(1).ok_or(Error::Limit)?;
                inner.queue_reserved = inner
                    .queue_reserved
                    .checked_sub(self.mailbox.limits.max_access_unit_bytes)
                    .ok_or(Error::Output)?;
                return Ok(Some(packet));
            }
            let remaining = self
                .control
                .deadline
                .saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Deadline);
            }
            inner = match self.mailbox.changed.wait_timeout(inner, remaining) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
    /// Stops submission and requests completion of every accepted picture.
    pub fn start_drain(&mut self) -> Result<(), Error> {
        let result = self.start_drain_inner();
        self.record(result)
    }
    fn start_drain_inner(&mut self) -> Result<(), Error> {
        self.check()?;
        {
            let mut inner = lock(&self.mailbox.inner);
            if inner.phase != Phase::Running {
                return Err(Error::State);
            }
            inner.phase = Phase::Draining;
        }
        self.native.as_mut().ok_or(Error::State)?.complete(None)?;
        self.check()
    }
    /// Validates exact submitted/output accounting and consumes the session.
    pub fn complete(mut self) -> Result<Completion, Error> {
        self.check()?;
        let identity = self.identity()?;
        let result = self.seal_output(identity)?;
        self.registration.retire();
        if let Some(mut native) = self.native.take() {
            native.close();
        }
        Ok(result)
    }

    fn seal_output(&self, identity: Identity) -> Result<Completion, Error> {
        let mut inner = lock(&self.mailbox.inner);
        self.mailbox.check(&inner)?;
        if inner.phase != Phase::Drained
            || !inner.slots.is_empty()
            || inner.accepted != inner.completed
            || inner.accepted != inner.delivered
            || inner.unclaimed_bytes != 0
            || inner.queue_reserved != 0
        {
            return Err(Error::Output);
        }
        let config = inner.config.take().ok_or(Error::Output)?;
        inner.phase = Phase::Closed;
        Ok(Completion {
            config,
            identity,
            settings: self.settings,
            frames: inner.accepted,
            bytes: inner.total_bytes,
        })
    }
}

impl<B: Backend> Drop for Session<B> {
    fn drop(&mut self) {
        self.registration.retire();
        if let Some(mut native) = self.native.take() {
            native.close();
        }
    }
}

/// One owner-thread-only native session.
///
/// Framework calls are synchronous but callbacks may be concurrent or reentrant.
/// A deadline cannot interrupt such a call or guarantee native teardown time.
///
/// ```compile_fail
/// use coreav_backend_apple::videotoolbox::Encoder;
/// fn requires_send<T: Send>() {}
/// requires_send::<Encoder>();
/// ```
///
/// ```compile_fail
/// use coreav_backend_apple::videotoolbox::Encoder;
/// fn requires_sync<T: Sync>() {}
/// requires_sync::<Encoder>();
/// ```
pub struct Encoder(Session<sys::Native>);

impl Encoder {
    /// Creates, prepares and checks one session before any source is accepted.
    pub fn new(settings: Settings, limits: Limits, control: Control) -> Result<Self, Error> {
        let native_control = control.clone();
        Session::new(settings, limits, control, |key| {
            sys::Native::new(settings, key, limits, native_control)
        })
        .map(Self)
    }
    /// Queries the actual implementation rather than the requested preference.
    pub fn identity(&self) -> Result<Identity, Error> {
        self.0.identity()
    }
    /// Copies one source picture, returning backpressure before any native submission.
    pub fn submit(&mut self, frame: Frame<'_>, pts: Time, duration: Time) -> Result<(), Error> {
        self.0.submit(frame, pts, duration)
    }
    /// Returns the oldest complete output, finishing through that time if needed.
    pub fn pull(&mut self) -> Result<Option<Packet>, Error> {
        self.0.pull()
    }
    /// Stops submission and requests completion of all pending pictures.
    pub fn start_drain(&mut self) -> Result<(), Error> {
        self.0.start_drain()
    }
    /// Returns final configuration only after exact frame accounting and consumed output.
    pub fn complete(self) -> Result<Completion, Error> {
        self.0.complete()
    }
}

#[cfg(test)]
mod tests;
