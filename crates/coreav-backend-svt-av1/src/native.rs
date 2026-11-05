//! The private C boundary. All submitted storage survives native destruction.

use std::{
    ffi::{CStr, c_char, c_void},
    ptr::{self, NonNull},
};

use crate::{
    Error,
    model::{MAX_PACKET_BYTES, Retention, Settings},
    retained::Retained,
};

const EMPTY: u32 = 0x8000_2033;
pub(crate) const EOS: u32 = 1;

#[cfg(test)]
static HEADER_DROPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static PLANE_DROPS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static OPEN_ATTEMPTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static LIVE_SESSIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static PEAK_SESSIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(test)]
static RETAINED_SETS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[repr(C)]
struct Config {
    width: u32,
    height: u32,
    rate_num: u32,
    rate_den: u32,
    preset: u32,
    crf: u32,
    keyint: u32,
    primaries: u32,
    transfer: u32,
    matrix: u32,
    full_range: u32,
    chroma_position: u32,
}

#[repr(C)]
struct Packet {
    data: *const u8,
    length: u32,
    capacity: u32,
    pts: i64,
    dts: i64,
    flags: u32,
    picture_type: u32,
    lease: *mut c_void,
}

const _: () = {
    assert!(size_of::<Config>() == 48);
    assert!(size_of::<Packet>() == 48);
    assert!(std::mem::offset_of!(Packet, pts) == 16);
    assert!(std::mem::offset_of!(Packet, lease) == 40);
};

unsafe extern "C" {
    fn coreav_svt_version() -> *const c_char;
    fn coreav_svt_input_bytes() -> usize;
    fn coreav_svt_open(config: *const Config, status: *mut u32) -> *mut c_void;
    fn coreav_svt_input_new(
        data: *mut u8,
        width: u32,
        height: u32,
        identity: i64,
        eof: u32,
    ) -> *mut c_void;
    fn coreav_svt_input_free(input: *mut c_void);
    fn coreav_svt_send(handle: *mut c_void, input: *mut c_void) -> u32;
    fn coreav_svt_receive(handle: *mut c_void, done: u8, packet: *mut Packet) -> u32;
    fn coreav_svt_packet_release(lease: *mut c_void);
    fn coreav_svt_close(handle: *mut c_void) -> u32;
}

pub(crate) fn version() -> Result<String, Error> {
    let pointer = unsafe { coreav_svt_version() };
    if pointer.is_null() {
        return Err(Error::Output("SVT-AV1 returned no version"));
    }
    let version = unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(|_| Error::Output("SVT-AV1 returned an invalid version"))?;
    if !matches!(version, "4.2.0" | "v4.2.0") {
        return Err(Error::Unsupported(
            "SVT-AV1 differs from the verified native pin",
        ));
    }
    Ok(version.to_owned())
}

pub(crate) fn retention(settings: Settings, budget: u64) -> Result<Retention, Error> {
    let input = unsafe { coreav_svt_input_bytes() };
    if input == 0 || input > 4096 {
        return Err(Error::Output("invalid native input allocation footprint"));
    }
    Retention::new(
        settings,
        budget,
        input,
        size_of::<Input>(),
        size_of::<Session>(),
    )
}

struct Input {
    pointer: NonNull<c_void>,
    bytes: Vec<u8>,
}

impl Input {
    fn picture(mut bytes: Vec<u8>, settings: Settings, identity: u32) -> Result<Self, Error> {
        if bytes.len() != settings.raw_bytes() || bytes.capacity() != settings.raw_bytes() {
            return Err(Error::Invalid(
                "AV1 input must own exactly its packed allocation",
            ));
        }
        let pointer = unsafe {
            coreav_svt_input_new(
                bytes.as_mut_ptr(),
                settings.width(),
                settings.height(),
                i64::from(identity),
                0,
            )
        };
        Ok(Self {
            pointer: NonNull::new(pointer)
                .ok_or(Error::Limit("cannot allocate native AV1 input header"))?,
            bytes,
        })
    }

    fn eof() -> Result<Self, Error> {
        let pointer = unsafe { coreav_svt_input_new(ptr::null_mut(), 0, 0, 0, 1) };
        Ok(Self {
            pointer: NonNull::new(pointer)
                .ok_or(Error::Limit("cannot allocate native AV1 EOF header"))?,
            bytes: Vec::new(),
        })
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        unsafe { coreav_svt_input_free(self.pointer.as_ptr()) };
        #[cfg(test)]
        {
            HEADER_DROPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self.bytes.is_empty() {
                PLANE_DROPS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }
}

struct Lease(NonNull<c_void>);

impl Drop for Lease {
    fn drop(&mut self) {
        unsafe { coreav_svt_packet_release(self.0.as_ptr()) };
    }
}

/// Native assertions and bytes, not codec-validated samples.
pub(crate) struct RawPacket {
    pub(crate) data: Vec<u8>,
    pub(crate) pts: i64,
    pub(crate) dts: i64,
    pub(crate) flags: u32,
    pub(crate) picture_type: u32,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Input,
    Drain,
    End,
    Failed,
    Destroyed,
}

pub(crate) struct Session {
    settings: Settings,
    plan: Retention,
    handle: Option<NonNull<c_void>>,
    retained: Option<Retained<Input>>,
    submitted: u32,
    phase: Phase,
}

impl Session {
    pub(crate) fn new(settings: Settings, budget: u64) -> Result<Self, Error> {
        version()?;
        let plan = retention(settings, budget)?;
        #[cfg(test)]
        assert_eq!(RETAINED_SETS.load(std::sync::atomic::Ordering::SeqCst), 0);
        let mut inputs = Vec::new();
        let count = usize::try_from(plan.pictures())
            .map_err(|_| Error::Limit("AV1 session count overflows"))?;
        inputs
            .try_reserve_exact(count)
            .map_err(|_| Error::Limit("cannot allocate retained AV1 slots"))?;
        if inputs.capacity() != count {
            return Err(Error::Limit(
                "retained AV1 slot allocation exceeds its footprint",
            ));
        }
        let eof = Input::eof()?;
        let (rate_num, rate_den) = settings.rate();
        let color = settings.color();
        let config = Config {
            width: settings.width(),
            height: settings.height(),
            rate_num,
            rate_den,
            preset: u32::from(settings.preset()),
            crf: u32::from(settings.crf()),
            keyint: settings.keyint(),
            primaries: u32::from(color.primaries()),
            transfer: u32::from(color.transfer()),
            matrix: u32::from(color.matrix()),
            full_range: u32::from(color.full_range()),
            chroma_position: u32::from(color.chroma_position()),
        };
        let mut status = 0;
        #[cfg(test)]
        OPEN_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let pointer = unsafe { coreav_svt_open(&config, &mut status) };
        if status != 0 {
            return Err(Error::Native(status));
        }
        let handle = NonNull::new(pointer)
            .ok_or(Error::Output("native initialization returned no handle"))?;
        #[cfg(test)]
        {
            let live = LIVE_SESSIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            PEAK_SESSIONS.fetch_max(live, std::sync::atomic::Ordering::SeqCst);
            RETAINED_SETS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(Self {
            settings,
            plan,
            handle: Some(handle),
            retained: Some(Retained::new(inputs, eof)),
            submitted: 0,
            phase: Phase::Input,
        })
    }

    pub(crate) fn plan(&self) -> Retention {
        self.plan
    }

    pub(crate) fn submitted(&self) -> u32 {
        self.submitted
    }

    pub(crate) fn retained_bytes(&mut self) -> Result<u64, Error> {
        let (inputs, _) = self
            .retained
            .as_mut()
            .and_then(Retained::get)
            .ok_or(Error::Invalid("AV1 retained owner is gone"))?;
        if inputs.capacity() != self.plan.pictures() as usize
            || inputs.len() != self.submitted as usize
            || inputs.iter().any(|input| {
                input.bytes.len() != self.settings.raw_bytes()
                    || input.bytes.capacity() != self.settings.raw_bytes()
            })
        {
            return Err(Error::Output(
                "AV1 retained allocations changed their exact capacities",
            ));
        }
        self.plan.peak_bytes(self.submitted)
    }

    fn handle(&self) -> Result<NonNull<c_void>, Error> {
        self.handle
            .ok_or(Error::Invalid("the AV1 session is destroyed"))
    }

    pub(crate) fn submit(&mut self, bytes: Vec<u8>) -> Result<(), Error> {
        self.submit_with(bytes, |handle, input| unsafe {
            coreav_svt_send(handle, input)
        })
    }

    fn submit_with(
        &mut self,
        bytes: Vec<u8>,
        send: impl FnOnce(*mut c_void, *mut c_void) -> u32,
    ) -> Result<(), Error> {
        if self.phase != Phase::Input {
            return Err(Error::Invalid("AV1 session is not accepting pictures"));
        }
        if self.submitted >= self.plan.pictures() {
            return Err(Error::Limit(
                "AV1 session needs drain before another picture",
            ));
        }
        let input = match Input::picture(bytes, self.settings, self.submitted) {
            Ok(input) => input,
            Err(error) => {
                self.phase = Phase::Failed;
                return Err(error);
            }
        };
        let pointer = input.pointer;
        let handle = self.handle()?;
        let (inputs, _) = self
            .retained
            .as_mut()
            .and_then(Retained::get)
            .ok_or(Error::Invalid("AV1 input storage is gone"))?;
        inputs.push(input);
        self.submitted += 1;
        if let Err(error) = self.retained_bytes() {
            self.phase = Phase::Failed;
            return Err(error);
        }
        let status = send(handle.as_ptr(), pointer.as_ptr());
        if status != 0 {
            self.phase = Phase::Failed;
            return Err(Error::Native(status));
        }
        Ok(())
    }

    pub(crate) fn end_input(&mut self) -> Result<(), Error> {
        self.end_input_with(|handle, input| unsafe { coreav_svt_send(handle, input) })
    }

    fn end_input_with(
        &mut self,
        send: impl FnOnce(*mut c_void, *mut c_void) -> u32,
    ) -> Result<(), Error> {
        if self.phase != Phase::Input || self.submitted == 0 {
            self.phase = Phase::Failed;
            return Err(Error::Invalid(
                "AV1 EOF requires a nonempty accepting session",
            ));
        }
        let handle = self.handle()?;
        let (_, eof) = self
            .retained
            .as_mut()
            .and_then(Retained::get)
            .ok_or(Error::Invalid("AV1 EOF storage is gone"))?;
        self.phase = Phase::Drain;
        let status = send(handle.as_ptr(), eof.pointer.as_ptr());
        if status != 0 {
            self.phase = Phase::Failed;
            return Err(Error::Native(status));
        }
        Ok(())
    }

    pub(crate) fn receive(&mut self, packet_limit: usize) -> Result<Option<RawPacket>, Error> {
        self.receive_with(packet_limit, |handle, done, packet| unsafe {
            coreav_svt_receive(handle, done, packet)
        })
    }

    fn receive_with(
        &mut self,
        packet_limit: usize,
        receive: impl FnOnce(*mut c_void, u8, *mut Packet) -> u32,
    ) -> Result<Option<RawPacket>, Error> {
        if self.phase == Phase::End {
            return Ok(None);
        }
        if !matches!(self.phase, Phase::Input | Phase::Drain) {
            return Err(Error::Invalid("AV1 session cannot return more output"));
        }
        let mut packet = Packet {
            data: ptr::null(),
            length: 0,
            capacity: 0,
            pts: 0,
            dts: 0,
            flags: 0,
            picture_type: 0,
            lease: ptr::null_mut(),
        };
        let status = receive(
            self.handle()?.as_ptr(),
            u8::from(self.phase == Phase::Drain),
            &mut packet,
        );
        if status == EMPTY && self.phase == Phase::Input {
            return Ok(None);
        }
        if status != 0 {
            self.phase = Phase::Failed;
            return Err(Error::Native(status));
        }
        let Some(lease) = NonNull::new(packet.lease).map(Lease) else {
            self.phase = Phase::Failed;
            return Err(Error::Output("native AV1 output has no release lease"));
        };
        let result = copy_packet(&packet, packet_limit);
        drop(lease);
        match result {
            Ok(output) => {
                if output.flags & EOS != 0 {
                    if self.phase != Phase::Drain {
                        self.phase = Phase::Failed;
                        return Err(Error::Output(
                            "native AV1 output ended before EOF submission",
                        ));
                    }
                    self.phase = Phase::End;
                }
                Ok(Some(output))
            }
            Err(error) => {
                self.phase = Phase::Failed;
                Err(error)
            }
        }
    }

    pub(crate) fn ended(&self) -> bool {
        self.phase == Phase::End
    }

    pub(crate) fn destroy(mut self) -> Result<(), Error> {
        self.close()
    }

    fn close(&mut self) -> Result<(), Error> {
        self.close_with(|handle| {
            let status = unsafe { coreav_svt_close(handle) };
            #[cfg(test)]
            if status == 0 {
                LIVE_SESSIONS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            status
        })
    }

    fn close_with(&mut self, destroy: impl FnOnce(*mut c_void) -> u32) -> Result<(), Error> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let retained = self
            .retained
            .take()
            .ok_or(Error::Invalid("native AV1 owner is gone"))?;
        let result = retained.release_after(|| {
            let status = destroy(handle.as_ptr());
            if status == 0 {
                Ok(())
            } else {
                Err(Error::Native(status))
            }
        });
        #[cfg(test)]
        if result.is_ok() {
            RETAINED_SETS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        self.phase = if result.is_ok() {
            Phase::Destroyed
        } else {
            Phase::Failed
        };
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn copy_packet(packet: &Packet, packet_limit: usize) -> Result<RawPacket, Error> {
    let length = usize::try_from(packet.length)
        .map_err(|_| Error::Limit("native AV1 output size overflows"))?;
    if packet.flags & 0xffff_fff0 != 0 {
        return Err(Error::Native(packet.flags & 0xffff_fff0));
    }
    if packet.length > packet.capacity {
        return Err(Error::Output(
            "native AV1 packet exceeds its reported buffer",
        ));
    }
    if length > packet_limit || length > MAX_PACKET_BYTES || length > isize::MAX as usize {
        return Err(Error::Limit("native AV1 packet exceeds its byte limit"));
    }
    if length == 0 && packet.flags & EOS == 0 {
        return Err(Error::Output(
            "native AV1 returned an empty nonterminal packet",
        ));
    }
    let mut data = Vec::new();
    if length != 0 {
        if packet.data.is_null() {
            return Err(Error::Output("native AV1 output has no payload pointer"));
        }
        data.try_reserve_exact(length)
            .map_err(|_| Error::Limit("cannot copy native AV1 output"))?;
        let source = unsafe { std::slice::from_raw_parts(packet.data, length) };
        data.extend_from_slice(source);
    }
    Ok(RawPacket {
        data,
        pts: packet.pts,
        dts: packet.dts,
        flags: packet.flags,
        picture_type: packet.picture_type,
    })
}
