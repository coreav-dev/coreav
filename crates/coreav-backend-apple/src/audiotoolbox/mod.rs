//! Bounded synchronous AAC-LC encoding through Apple's direct AudioCodec API.
//!
//! No callback is registered. Appending copies accepted input into the codec's
//! buffer during the call. Synchronous framework calls cannot be interrupted.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use crate::aac::{self, BatchLimits, Config, PacketBatch, PacketDescription, Trim};

#[cfg(target_os = "macos")]
mod sys;

/// The closed set of native operations exposed in failure diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Query the instantiated component's description.
    Describe,
    /// Query the component version.
    Version,
    /// Create a component instance.
    Create,
    /// Initialize configured stream formats.
    Initialize,
    /// Read a numeric property identifier.
    GetProperty(u32),
    /// Set a numeric property identifier.
    SetProperty(u32),
    /// Query a numeric property's size and writability.
    PropertyInfo(u32),
    /// Append input bytes or the EOF marker.
    Append,
    /// Produce encoded output packets.
    Produce,
    /// Dispose of the component instance.
    Dispose,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GetProperty(id) => write!(f, "get property {id:08x}"),
            Self::SetProperty(id) => write!(f, "set property {id:08x}"),
            Self::PropertyInfo(id) => write!(f, "query property {id:08x}"),
            _ => f.write_str(match self {
                Self::Describe => "describe component",
                Self::Version => "query component version",
                Self::Create => "create component",
                Self::Initialize => "initialize codec",
                Self::Append => "append input",
                Self::Produce => "produce output",
                Self::Dispose => "dispose component",
                _ => "property operation",
            }),
        }
    }
}

/// An encoder operation that could not complete safely.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The selected platform component is unavailable.
    Unavailable,
    /// A PCM format, bitrate, quality, or chunk is invalid.
    Input,
    /// The session is no longer in the state required by the operation.
    State,
    /// A caller-supplied resource bound was exceeded.
    Limit,
    /// Rust-owned output storage could not be reserved.
    Allocation,
    /// The shared cancellation flag was set.
    Cancelled,
    /// The deadline expired before or after a synchronous call.
    Deadline,
    /// The framework returned an OSStatus error.
    Framework {
        /// The native operation, without untrusted exception text.
        operation: Operation,
        /// Apple's signed OSStatus value.
        status: i32,
    },
    /// An effective control does not match the required value.
    Setting {
        /// Numeric AudioCodec property identifier.
        property: u32,
        /// Required value.
        requested: u32,
        /// Value reported by the initialized codec.
        actual: u32,
    },
    /// Framework output, settings, progress, or counts are invalid.
    Output,
    /// AAC output metadata failed validation.
    Metadata(aac::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Framework { operation, status } => write!(
                f,
                "AudioToolbox failed to {operation} with OSStatus {status}"
            ),
            Self::Setting {
                property,
                requested,
                actual,
            } => write!(
                f,
                "AudioToolbox property {property:08x} is {actual}, requested {requested}"
            ),
            Self::Metadata(error) => error.fmt(f),
            _ => f.write_str(match self {
                Self::Unavailable => "the Apple AAC encoder is unavailable",
                Self::Input => "invalid AAC encoder input or settings",
                Self::State => "invalid AAC encoder session state",
                Self::Limit => "AAC encoding exceeds its configured limit",
                Self::Allocation => "cannot reserve AAC encoder storage",
                Self::Cancelled => "AAC encoding was cancelled",
                Self::Deadline => "AAC encoding deadline expired",
                _ => "invalid AudioToolbox output",
            }),
        }
    }
}
impl std::error::Error for Error {}
impl From<aac::Error> for Error {
    fn from(value: aac::Error) -> Self {
        Self::Metadata(value)
    }
}

/// Checked settings. The codec must retain the exact PCM rate and channel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    sample_rate: u32,
    channels: u16,
    bitrate: u32,
    quality: u32,
}

impl Settings {
    /// Selects mono or stereo AAC-LC, indexed rate, CBR, and quality 0 through 127.
    pub fn new(sample_rate: u32, channels: u16, bitrate: u32, quality: u32) -> Result<Self, Error> {
        Config::for_pcm(sample_rate, channels).map_err(|_| Error::Input)?;
        if bitrate == 0 || quality > 127 {
            return Err(Error::Input);
        }
        Ok(Self {
            sample_rate,
            channels,
            bitrate,
            quality,
        })
    }
    /// Returns the unchanged sample rate.
    pub fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    /// Returns one for mono, two for left-right stereo.
    pub fn channels(self) -> u16 {
        self.channels
    }
    /// Returns the exact requested and checked CBR target.
    pub fn bitrate(self) -> u32 {
        self.bitrate
    }
    /// Returns the exact requested and checked codec quality setting.
    pub fn quality(self) -> u32 {
        self.quality
    }
}

/// Per-session bounds, separate from the framework's own allocations.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Most sample frames per channel in one append call.
    pub max_chunk_frames: u32,
    /// Most accepted PCM sample frames per channel in the session.
    pub max_total_frames: u64,
    /// Most emitted packet bytes in the session.
    pub max_output_bytes: u64,
    /// Most emitted AAC packets in the session.
    pub max_packets: u64,
    /// Most bytes in one AAC packet and one native output buffer.
    pub max_packet_bytes: usize,
    /// Most bytes in a final codec magic cookie.
    pub max_cookie_bytes: usize,
    /// Maximum leading trim and maximum trailing trim, each in sample frames.
    pub max_trim_frames: u32,
}

impl Limits {
    fn check(self) -> Result<(), Error> {
        if self.max_chunk_frames == 0
            || self.max_packet_bytes == 0
            || self.max_packet_bytes > u32::MAX as usize
            || self.max_cookie_bytes == 0
        {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

/// A shared cancellation flag and one deadline for the entire session.
#[derive(Clone, Debug)]
pub struct Control {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl Control {
    /// Creates the control without extending its deadline on subsequent calls.
    pub fn new(deadline: Instant) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline,
        }
    }
    /// Cancels future work; this cannot interrupt a synchronous framework call.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
    fn check(&self) -> Result<(), Error> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Deadline);
        }
        Ok(())
    }
}

/// The actual instantiated AudioComponent identity, not a requested preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Component type, as a big-endian FourCC integer.
    pub kind: u32,
    /// Codec subtype, as a big-endian FourCC integer.
    pub subtype: u32,
    /// Manufacturer, as a big-endian FourCC integer.
    pub manufacturer: u32,
    /// AudioComponent's documented 0xMMMMmmDD version value.
    pub version: u32,
}

impl Identity {
    /// Reports the selected Apple software codec as software, otherwise unknown.
    pub fn hardware_accelerated(self) -> Option<bool> {
        if self.kind == u32::from_be_bytes(*b"aenc")
            && self.subtype == u32::from_be_bytes(*b"aac ")
            && self.manufacturer == u32::from_be_bytes(*b"appl")
        {
            Some(false)
        } else {
            None
        }
    }
}

/// What the caller may do after receiving a checked output batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// More output may be available without appending input.
    More,
    /// Append more PCM or signal end of input.
    NeedInput,
    /// The EOF marker was reached; request the final checked metadata.
    Drained,
}

/// One owned output batch and the codec's validated progress state.
#[derive(Debug)]
pub struct Output {
    /// Zero or one checked AAC packets.
    pub packets: PacketBatch,
    /// Next permitted operation.
    pub flow: Flow,
}

/// Checked final configuration and declared frame accounting after EOF.
#[derive(Debug)]
pub struct Completion {
    /// Validated configuration for a container sample entry.
    pub config: Config,
    /// Declared output trim, checked against actual emitted packet totals.
    pub trim: Trim,
    /// Total accepted PCM sample frames per channel.
    pub submitted_frames: u64,
    /// Total emitted AAC packet count.
    pub packet_count: u64,
    /// The instantiated encoder's identity.
    pub identity: Identity,
    /// Settings queried back from the initialized encoder.
    pub settings: Settings,
}

struct RawOutput {
    data: Vec<u8>,
    count: u32,
    description: PacketDescription,
    status: u32,
}

trait Backend {
    fn append(&mut self, pcm: &[f32]) -> Result<usize, Error>;
    fn eof(&mut self) -> Result<(), Error>;
    fn produce(&mut self, capacity: usize) -> Result<RawOutput, Error>;
    fn cookie(&mut self, limit: usize) -> Result<Vec<u8>, Error>;
    fn trim(&mut self) -> Result<(u32, u32), Error>;
    fn identity(&self) -> Identity;
    fn close(&mut self) -> Result<(), Error>;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Draining,
    Drained,
    Failed,
}

struct Session<B> {
    backend: B,
    settings: Settings,
    limits: Limits,
    control: Control,
    phase: Phase,
    submitted: u64,
    packets: u64,
    bytes: u64,
}

impl<B: Backend> Session<B> {
    fn new(
        backend: B,
        settings: Settings,
        limits: Limits,
        control: Control,
    ) -> Result<Self, Error> {
        control.check()?;
        limits.check()?;
        Ok(Self {
            backend,
            settings,
            limits,
            control,
            phase: Phase::Running,
            submitted: 0,
            packets: 0,
            bytes: 0,
        })
    }

    fn fail<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn call<T>(&mut self, f: impl FnOnce(&mut B) -> Result<T, Error>) -> Result<T, Error> {
        let check = self.control.check();
        self.fail(check)?;
        let result = f(&mut self.backend);
        let result = result.and_then(|value| self.control.check().map(|()| value));
        self.fail(result)
    }

    fn append(&mut self, pcm: &[f32], first_frame: u64) -> Result<usize, Error> {
        if self.phase != Phase::Running {
            return Err(Error::State);
        }
        let channels = usize::from(self.settings.channels);
        if first_frame != self.submitted || pcm.is_empty() || pcm.len() % channels != 0 {
            return self.fail(Err(Error::Input));
        }
        let frames = u64::try_from(pcm.len() / channels).map_err(|_| Error::Limit)?;
        if frames > u64::from(self.limits.max_chunk_frames)
            || self
                .submitted
                .checked_add(frames)
                .is_none_or(|total| total > self.limits.max_total_frames)
        {
            return self.fail(Err(Error::Limit));
        }
        let check = self.control.check();
        self.fail(check)?;
        if pcm.iter().any(|sample| !sample.is_finite()) {
            return self.fail(Err(Error::Input));
        }
        let consumed = self.call(|backend| backend.append(pcm))?;
        if consumed > pcm.len() / channels {
            return self.fail(Err(Error::Output));
        }
        self.submitted += consumed as u64;
        Ok(consumed)
    }

    fn start_drain(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Running {
            return Err(Error::State);
        }
        self.call(Backend::eof)?;
        self.phase = Phase::Draining;
        Ok(())
    }

    fn pull(&mut self) -> Result<Output, Error> {
        if !matches!(self.phase, Phase::Running | Phase::Draining) {
            return Err(Error::State);
        }
        let capacity = self.limits.max_packet_bytes;
        let raw = self.call(|backend| backend.produce(capacity))?;
        let result = self.output(raw);
        self.fail(result)
    }

    fn output(&mut self, raw: RawOutput) -> Result<Output, Error> {
        let flow = match raw.status {
            2 if self.phase == Phase::Running => Flow::NeedInput,
            2 | 3 => Flow::More,
            4 if self.phase == Phase::Running => Flow::NeedInput,
            5 if self.phase == Phase::Draining => Flow::Drained,
            _ => return Err(Error::Output),
        };
        if raw.count > 1 || (flow == Flow::More && raw.count == 0) {
            return Err(Error::Output);
        }
        let descriptions = if raw.count == 0 {
            &[][..]
        } else {
            std::slice::from_ref(&raw.description)
        };
        let packets = PacketBatch::copy_from(
            &raw.data,
            descriptions,
            BatchLimits {
                max_buffer_bytes: self.limits.max_packet_bytes,
                max_packet_bytes: self.limits.max_packet_bytes,
                max_packets: 1,
            },
        )?;
        let count = self
            .packets
            .checked_add(u64::from(raw.count))
            .ok_or(Error::Limit)?;
        let byte_count = packets
            .packets()
            .map(|packet| packet.len() as u64)
            .sum::<u64>();
        let bytes = self.bytes.checked_add(byte_count).ok_or(Error::Limit)?;
        let maximum_frames = self
            .submitted
            .checked_add(u64::from(self.limits.max_trim_frames) * 2)
            .ok_or(Error::Limit)?;
        if count > self.limits.max_packets
            || bytes > self.limits.max_output_bytes
            || count
                .checked_mul(1024)
                .is_none_or(|frames| frames > maximum_frames)
        {
            return Err(Error::Limit);
        }
        self.packets = count;
        self.bytes = bytes;
        if flow == Flow::Drained {
            self.phase = Phase::Drained;
        }
        Ok(Output { packets, flow })
    }

    fn complete(mut self) -> Result<Completion, Error> {
        if self.phase != Phase::Drained {
            return Err(Error::State);
        }
        let limit = self.limits.max_cookie_bytes;
        let cookie = self.call(|backend| backend.cookie(limit))?;
        if cookie.len() > limit {
            return Err(Error::Limit);
        }
        let config =
            Config::from_cookie(&cookie, self.settings.sample_rate, self.settings.channels)?;
        let (leading, trailing) = self.call(Backend::trim)?;
        let trim = Trim::validate(
            self.packets,
            self.submitted,
            leading,
            trailing,
            self.limits.max_trim_frames,
        )?;
        let identity = self.backend.identity();
        self.call(Backend::close)?;
        Ok(Completion {
            config,
            trim,
            submitted_frames: self.submitted,
            packet_count: self.packets,
            identity,
            settings: self.settings,
        })
    }
}

/// A non-Send, non-Sync owner of one initialized Apple AAC encoder.
///
/// No callback, frame reference, or output queue is retained. The framework's
/// internal allocations and synchronous-call duration are not enforceably bounded.
///
/// ```compile_fail
/// fn requires_send<T: Send>() {}
/// requires_send::<coreav_backend_apple::audiotoolbox::Encoder>();
/// ```
///
/// ```compile_fail
/// fn requires_sync<T: Sync>() {}
/// requires_sync::<coreav_backend_apple::audiotoolbox::Encoder>();
/// ```
#[cfg(target_os = "macos")]
pub struct Encoder(Session<sys::Instance>);

#[cfg(target_os = "macos")]
impl Encoder {
    /// Opens Apple's AAC-LC component and verifies the effective format/settings.
    pub fn new(settings: Settings, limits: Limits, control: Control) -> Result<Self, Error> {
        control.check()?;
        limits.check()?;
        let backend = sys::Instance::new(settings, &control)?;
        control.check()?;
        let mut session = Session::new(backend, settings, limits, control)?;
        let maximum = session.call(|backend| backend.u32(*b"pakb"))?;
        if maximum == 0 || maximum as usize > limits.max_packet_bytes {
            return Err(Error::Limit);
        }
        Ok(Self(session))
    }
    /// Appends finite interleaved PCM, returning accepted sample frames per channel.
    ///
    /// Partial consumption is explicit. Retry only the unconsumed suffix, with its
    /// corresponding `first_frame`. No borrowed data is retained after the call.
    pub fn append(&mut self, pcm: &[f32], first_frame: u64) -> Result<usize, Error> {
        self.0.append(pcm, first_frame)
    }
    /// Signals EOF once; further PCM appends are refused.
    pub fn start_drain(&mut self) -> Result<(), Error> {
        self.0.start_drain()
    }
    /// Pulls at most one packet, validating bytes, counts, ordering, and progress.
    pub fn pull(&mut self) -> Result<Output, Error> {
        self.0.pull()
    }
    /// Consumes a drained encoder and validates its final cookie and trim counts.
    pub fn complete(self) -> Result<Completion, Error> {
        self.0.complete()
    }
    /// Returns the actual component identity, including the selected software manufacturer.
    pub fn identity(&self) -> Identity {
        self.0.backend.identity()
    }
}

#[cfg(target_os = "macos")]
impl Backend for sys::Instance {
    fn close(&mut self) -> Result<(), Error> {
        self.close()
    }
    fn append(&mut self, pcm: &[f32]) -> Result<usize, Error> {
        self.append(pcm)
    }
    fn eof(&mut self) -> Result<(), Error> {
        self.eof()
    }
    fn produce(&mut self, capacity: usize) -> Result<RawOutput, Error> {
        self.produce(capacity)
    }
    fn cookie(&mut self, limit: usize) -> Result<Vec<u8>, Error> {
        self.cookie(limit)
    }
    fn trim(&mut self) -> Result<(u32, u32), Error> {
        self.trim()
    }
    fn identity(&self) -> Identity {
        self.identity()
    }
}

#[cfg(test)]
mod tests;
