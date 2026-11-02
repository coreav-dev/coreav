#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use super::{Control, Error, Identity, Limits, Operation, RawOutput, Settings};
use crate::aac::PacketDescription;

const AAC: u32 = u32::from_be_bytes(*b"aac ");
const PCM: u32 = u32::from_be_bytes(*b"lpcm");
const ENCODER: u32 = u32::from_be_bytes(*b"aenc");
const APPLE: u32 = u32::from_be_bytes(*b"appl");

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Description {
    kind: u32,
    subtype: u32,
    manufacturer: u32,
    flags: u32,
    mask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Format {
    rate: f64,
    codec: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}

#[repr(C)]
#[derive(Default)]
struct Packet {
    offset: i64,
    frames: u32,
    bytes: u32,
}

#[repr(C)]
#[derive(Default)]
struct Prime {
    leading: u32,
    trailing: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioComponentFindNext(previous: *mut c_void, desc: *const Description) -> *mut c_void;
    fn AudioComponentGetDescription(component: *mut c_void, desc: *mut Description) -> i32;
    fn AudioComponentGetVersion(component: *mut c_void, version: *mut u32) -> i32;
    fn AudioComponentInstanceNew(component: *mut c_void, instance: *mut *mut c_void) -> i32;
    fn AudioComponentInstanceGetComponent(instance: *mut c_void) -> *mut c_void;
    fn AudioComponentInstanceDispose(instance: *mut c_void) -> i32;
    fn AudioCodecSetProperty(
        instance: *mut c_void,
        property: u32,
        size: u32,
        data: *const c_void,
    ) -> i32;
    fn AudioCodecGetPropertyInfo(
        instance: *mut c_void,
        property: u32,
        size: *mut u32,
        writable: *mut u8,
    ) -> i32;
    fn AudioCodecGetProperty(
        instance: *mut c_void,
        property: u32,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioCodecInitialize(
        instance: *mut c_void,
        input: *const Format,
        output: *const Format,
        cookie: *const c_void,
        cookie_len: u32,
    ) -> i32;
    fn AudioCodecAppendInputData(
        instance: *mut c_void,
        data: *const c_void,
        bytes: *mut u32,
        packets: *mut u32,
        desc: *const Packet,
    ) -> i32;
    fn AudioCodecProduceOutputPackets(
        instance: *mut c_void,
        data: *mut c_void,
        bytes: *mut u32,
        packets: *mut u32,
        desc: *mut Packet,
        status: *mut u32,
    ) -> i32;
}

pub(super) struct Instance {
    handle: NonNull<c_void>,
    channels: u32,
    identity: Identity,
    control: Control,
    disposed: bool,
    source_frames: u64,
    max_trim_frames: u32,
    drain: Option<Drain>,
}

#[derive(Clone, Copy)]
struct Drain {
    remaining: u32,
    prime: (u32, u32),
    trailing: u32,
    eof: bool,
}

impl Drain {
    fn new(source: u64, prime: (u32, u32), limit: u32) -> Result<Self, Error> {
        if prime.0 > limit || prime.1 > limit {
            return Err(Error::Limit);
        }
        let total = source
            .checked_add(u64::from(prime.0))
            .and_then(|n| n.checked_add(u64::from(prime.1)))
            .ok_or(Error::Limit)?;
        let alignment = (1024 - total % 1024) % 1024;
        let trailing = u32::try_from(u64::from(prime.1) + alignment).map_err(|_| Error::Limit)?;
        if trailing > limit {
            return Err(Error::Limit);
        }
        let remaining = prime.0.checked_add(trailing).ok_or(Error::Limit)?;
        Ok(Self {
            remaining,
            prime,
            trailing,
            eof: false,
        })
    }

    fn consume(&mut self, frames: usize) -> Result<(), Error> {
        if self.eof {
            return Err(Error::State);
        }
        self.remaining = self
            .remaining
            .checked_sub(u32::try_from(frames).map_err(|_| Error::Output)?)
            .ok_or(Error::Output)?;
        Ok(())
    }

    fn mark_eof(&mut self) -> Result<(), Error> {
        if self.remaining != 0 || self.eof {
            return Err(Error::State);
        }
        self.eof = true;
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        if !self.disposed {
            unsafe { AudioComponentInstanceDispose(self.handle.as_ptr()) };
        }
    }
}

fn checked(status: i32, operation: Operation) -> Result<(), Error> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Framework { operation, status })
    }
}

fn successful_handle(status: i32, handle: *mut c_void) -> Result<NonNull<c_void>, Error> {
    checked(status, Operation::Create)?;
    NonNull::new(handle).ok_or(Error::Output)
}

fn invoke(control: &Control, operation: Operation, f: impl FnOnce() -> i32) -> Result<(), Error> {
    control.check()?;
    let result = checked(f(), operation);
    control.check()?;
    result
}

impl Instance {
    pub(super) fn new(
        settings: Settings,
        limits: Limits,
        control: &Control,
    ) -> Result<Self, Error> {
        let requested = Description {
            kind: ENCODER,
            subtype: AAC,
            manufacturer: APPLE,
            ..Description::default()
        };
        control.check()?;
        let component = unsafe { AudioComponentFindNext(ptr::null_mut(), &requested) };
        control.check()?;
        if component.is_null() {
            return Err(Error::Unavailable);
        }
        let mut actual = Description::default();
        invoke(control, Operation::Describe, || unsafe {
            AudioComponentGetDescription(component, &mut actual)
        })?;
        if actual.kind != ENCODER
            || actual.subtype != AAC
            || actual.manufacturer != APPLE
            || actual.flags & 12 != 0
        {
            return Err(Error::Unavailable);
        }
        let mut version = 0;
        invoke(control, Operation::Version, || unsafe {
            AudioComponentGetVersion(component, &mut version)
        })?;
        let mut handle = ptr::null_mut();
        control.check()?;
        let status = unsafe { AudioComponentInstanceNew(component, &mut handle) };
        let handle = successful_handle(status, handle)?;
        let mut owner = Self {
            handle,
            channels: u32::from(settings.channels),
            identity: Identity {
                kind: actual.kind,
                subtype: actual.subtype,
                manufacturer: actual.manufacturer,
                version,
            },
            control: control.clone(),
            disposed: false,
            source_frames: 0,
            max_trim_frames: limits.max_trim_frames,
            drain: None,
        };
        control.check()?;
        let instantiated = unsafe { AudioComponentInstanceGetComponent(owner.handle.as_ptr()) };
        control.check()?;
        if instantiated.is_null() {
            return Err(Error::Output);
        }
        invoke(control, Operation::Describe, || unsafe {
            AudioComponentGetDescription(instantiated, &mut actual)
        })?;
        invoke(control, Operation::Version, || unsafe {
            AudioComponentGetVersion(instantiated, &mut version)
        })?;
        if actual.kind != ENCODER
            || actual.subtype != AAC
            || actual.manufacturer != APPLE
            || actual.flags & 12 != 0
        {
            return Err(Error::Output);
        }
        owner.identity = Identity {
            kind: actual.kind,
            subtype: actual.subtype,
            manufacturer: actual.manufacturer,
            version,
        };
        let input = Format {
            rate: f64::from(settings.sample_rate),
            codec: PCM,
            flags: 9,
            bytes_per_packet: owner.channels * 4,
            frames_per_packet: 1,
            bytes_per_frame: owner.channels * 4,
            channels: owner.channels,
            bits: 32,
            reserved: 0,
        };
        let output = Format {
            rate: f64::from(settings.sample_rate),
            codec: AAC,
            frames_per_packet: 1024,
            channels: owner.channels,
            ..Format::default()
        };
        owner.set_format(*b"ifmt", &input)?;
        owner.set_format(*b"ofmt", &output)?;
        owner.set(*b"acbf", 0)?;
        owner.set(*b"brat", settings.bitrate)?;
        owner.set(*b"srcq", settings.quality)?;
        invoke(control, Operation::Initialize, || unsafe {
            AudioCodecInitialize(
                owner.handle.as_ptr(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                0,
            )
        })?;
        if owner.format(*b"ifmt")? != input {
            return Err(Error::Output);
        }
        let actual = owner.format(*b"ofmt")?;
        if actual.rate != output.rate
            || actual.codec != AAC
            || actual.channels != output.channels
            || actual.frames_per_packet != 1024
            || actual.flags != 0
        {
            return Err(Error::Output);
        }
        for (property, expected) in [
            (*b"brat", settings.bitrate),
            (*b"srcq", settings.quality),
            (*b"acbf", 0),
        ] {
            let actual = owner.u32(property)?;
            if actual != expected {
                return Err(Error::Setting {
                    property: u32::from_be_bytes(property),
                    requested: expected,
                    actual,
                });
            }
        }
        Ok(owner)
    }

    fn set(&mut self, property: [u8; 4], value: u32) -> Result<(), Error> {
        let mut size = 0;
        let mut writable = 0;
        invoke(
            &self.control,
            Operation::PropertyInfo(u32::from_be_bytes(property)),
            || unsafe {
                AudioCodecGetPropertyInfo(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(property),
                    &mut size,
                    &mut writable,
                )
            },
        )?;
        if size != 4 {
            return Err(Error::Output);
        }
        #[cfg(test)]
        eprintln!(
            "AudioCodec scalar {:08x}: requested {value}, writable {writable}",
            u32::from_be_bytes(property)
        );
        if writable == 0 {
            return Ok(());
        }
        invoke(
            &self.control,
            Operation::SetProperty(u32::from_be_bytes(property)),
            || unsafe {
                AudioCodecSetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(property),
                    4,
                    (&value as *const u32).cast(),
                )
            },
        )?;
        #[cfg(test)]
        eprintln!(
            "AudioCodec scalar {:08x}: before initialization {}",
            u32::from_be_bytes(property),
            self.u32(property)?
        );
        Ok(())
    }

    fn set_format(&mut self, property: [u8; 4], value: &Format) -> Result<(), Error> {
        invoke(
            &self.control,
            Operation::SetProperty(u32::from_be_bytes(property)),
            || unsafe {
                AudioCodecSetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(property),
                    40,
                    (value as *const Format).cast(),
                )
            },
        )
    }

    fn format(&mut self, property: [u8; 4]) -> Result<Format, Error> {
        let mut value = Format::default();
        let mut size = 40;
        invoke(
            &self.control,
            Operation::GetProperty(u32::from_be_bytes(property)),
            || unsafe {
                AudioCodecGetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(property),
                    &mut size,
                    (&mut value as *mut Format).cast(),
                )
            },
        )?;
        if size != 40 {
            return Err(Error::Output);
        }
        Ok(value)
    }

    pub(super) fn u32(&mut self, property: [u8; 4]) -> Result<u32, Error> {
        let mut value = 0;
        let mut size = 4;
        invoke(
            &self.control,
            Operation::GetProperty(u32::from_be_bytes(property)),
            || unsafe {
                AudioCodecGetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(property),
                    &mut size,
                    (&mut value as *mut u32).cast(),
                )
            },
        )?;
        if size != 4 {
            return Err(Error::Output);
        }
        Ok(value)
    }

    pub(super) fn identity(&self) -> Identity {
        self.identity
    }

    pub(super) fn close(&mut self) -> Result<(), Error> {
        if self.disposed {
            return Err(Error::State);
        }
        let status = unsafe { AudioComponentInstanceDispose(self.handle.as_ptr()) };
        self.disposed = true;
        checked(status, Operation::Dispose)
    }

    pub(super) fn append(&mut self, samples: &[f32]) -> Result<usize, Error> {
        if self.drain.is_some() {
            return Err(Error::State);
        }
        let consumed = self.append_pcm(samples)?;
        self.source_frames = self
            .source_frames
            .checked_add(consumed as u64)
            .ok_or(Error::Limit)?;
        Ok(consumed)
    }

    fn append_pcm(&mut self, samples: &[f32]) -> Result<usize, Error> {
        let offered = u32::try_from(std::mem::size_of_val(samples)).map_err(|_| Error::Limit)?;
        let mut bytes = offered;
        let offered_frames =
            u32::try_from(samples.len() / self.channels as usize).map_err(|_| Error::Limit)?;
        let mut frames = offered_frames;
        invoke(&self.control, Operation::Append, || unsafe {
            AudioCodecAppendInputData(
                self.handle.as_ptr(),
                samples.as_ptr().cast(),
                &mut bytes,
                &mut frames,
                ptr::null(),
            )
        })?;
        #[cfg(test)]
        eprintln!("AudioCodec append: bytes {bytes}/{offered}, packets {frames}/{offered_frames}");
        if bytes > offered || bytes % (self.channels * 4) != 0 || frames > offered_frames {
            return Err(Error::Output);
        }
        usize::try_from(bytes / (self.channels * 4)).map_err(|_| Error::Output)
    }

    pub(super) fn eof(&mut self) -> Result<(), Error> {
        if self.drain.is_some() {
            return Err(Error::State);
        }
        self.drain = Some(Drain::new(
            self.source_frames,
            self.priming()?,
            self.max_trim_frames,
        )?);
        Ok(())
    }

    fn signal_eof(&mut self) -> Result<(), Error> {
        let mut bytes = 0;
        let mut packets = 0;
        let marker = 0_f32;
        invoke(&self.control, Operation::Append, || unsafe {
            AudioCodecAppendInputData(
                self.handle.as_ptr(),
                (&marker as *const f32).cast(),
                &mut bytes,
                &mut packets,
                ptr::null(),
            )
        })?;
        #[cfg(test)]
        eprintln!("AudioCodec EOF: bytes {bytes}, packets {packets}");
        if bytes != 0 || packets != 0 {
            return Err(Error::Output);
        }
        Ok(())
    }

    pub(super) fn produce(&mut self, capacity: usize) -> Result<RawOutput, Error> {
        if let Some(mut drain) = self.drain {
            if !drain.eof {
                if drain.remaining != 0 {
                    let zeros = [0.0_f32; 2048];
                    let frames = drain.remaining.min(1024);
                    let samples = zeros
                        .get(..(frames * self.channels) as usize)
                        .ok_or(Error::Output)?;
                    let consumed = self.append_pcm(samples)?;
                    drain.consume(consumed)?;
                }
                if drain.remaining == 0 {
                    self.signal_eof()?;
                    drain.mark_eof()?;
                }
                self.drain = Some(drain);
            }
        }
        let mut bytes = u32::try_from(capacity).map_err(|_| Error::Limit)?;
        let mut data = Vec::new();
        data.try_reserve_exact(capacity)
            .map_err(|_| Error::Allocation)?;
        data.resize(capacity, 0_u8);
        let mut count = 1;
        let mut description = Packet::default();
        let mut status = 0;
        invoke(&self.control, Operation::Produce, || unsafe {
            AudioCodecProduceOutputPackets(
                self.handle.as_ptr(),
                data.as_mut_ptr().cast(),
                &mut bytes,
                &mut count,
                &mut description,
                &mut status,
            )
        })?;
        #[cfg(test)]
        eprintln!(
            "AudioCodec output: status {status}, bytes {bytes}/{capacity}, packets {count}, offset {}, packet bytes {}, frames {}",
            description.offset, description.bytes, description.frames
        );
        if bytes as usize > data.len() || count > 1 {
            return Err(Error::Output);
        }
        data.truncate(bytes as usize);
        if status == 4 && count == 0 && self.drain.is_some_and(|drain| drain.eof) {
            status = 5;
        }
        Ok(RawOutput {
            data,
            count,
            description: PacketDescription {
                start_offset: description.offset,
                byte_size: description.bytes,
                variable_frames: description.frames,
            },
            status,
        })
    }

    pub(super) fn cookie(&mut self, limit: usize) -> Result<Vec<u8>, Error> {
        let mut size = 0;
        invoke(
            &self.control,
            Operation::PropertyInfo(u32::from_be_bytes(*b"kuki")),
            || unsafe {
                AudioCodecGetPropertyInfo(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(*b"kuki"),
                    &mut size,
                    ptr::null_mut(),
                )
            },
        )?;
        let capacity = usize::try_from(size).map_err(|_| Error::Output)?;
        if capacity == 0 || capacity > limit {
            return Err(Error::Limit);
        }
        let mut data = Vec::new();
        data.try_reserve_exact(capacity)
            .map_err(|_| Error::Allocation)?;
        data.resize(capacity, 0_u8);
        invoke(
            &self.control,
            Operation::GetProperty(u32::from_be_bytes(*b"kuki")),
            || unsafe {
                AudioCodecGetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(*b"kuki"),
                    &mut size,
                    data.as_mut_ptr().cast(),
                )
            },
        )?;
        if size as usize > capacity {
            return Err(Error::Output);
        }
        data.truncate(size as usize);
        Ok(data)
    }

    fn priming(&mut self) -> Result<(u32, u32), Error> {
        let mut prime = Prime::default();
        let mut size = 8;
        invoke(
            &self.control,
            Operation::GetProperty(u32::from_be_bytes(*b"prim")),
            || unsafe {
                AudioCodecGetProperty(
                    self.handle.as_ptr(),
                    u32::from_be_bytes(*b"prim"),
                    &mut size,
                    (&mut prime as *mut Prime).cast(),
                )
            },
        )?;
        if size != 8 {
            return Err(Error::Output);
        }
        Ok((prime.leading, prime.trailing))
    }

    pub(super) fn trim(&mut self) -> Result<(u32, u32), Error> {
        let drain = self.drain.filter(|drain| drain.eof).ok_or(Error::State)?;
        if self.priming()? != drain.prime || self.u32(*b"pad0")? != 0 {
            return Err(Error::Output);
        }
        Ok((drain.prime.0, drain.trailing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_padding_accounts_for_delay_and_packet_alignment() -> Result<(), Error> {
        for (source, prime, remaining, trailing) in [
            (5000, (2112, 0), 2168, 56),
            (0, (2112, 0), 3072, 960),
            (1024, (0, 0), 0, 0),
            (1023, (0, 0), 1, 1),
            (5000, (100, 20), 120, 20),
        ] {
            let drain = Drain::new(source, prime, 4096)?;
            assert_eq!(drain.remaining, remaining);
            assert_eq!(drain.trailing, trailing);
            assert_eq!(drain.prime, prime);
            assert!(!drain.eof);
            assert_eq!((source + u64::from(drain.remaining)) % 1024, 0);
            assert_eq!(
                source + u64::from(drain.remaining)
                    - u64::from(prime.0)
                    - u64::from(drain.trailing),
                source
            );
        }
        Ok(())
    }

    #[test]
    fn drain_padding_refuses_excess_trim_and_overflow() {
        for (source, prime, limit) in [
            (5000, (2112, 0), 2111),
            (5000, (0, 20), 19),
            (1023, (0, 0), 0),
            (u64::MAX, (1, 0), 4096),
            (0, (u32::MAX, u32::MAX), u32::MAX),
        ] {
            assert_eq!(Drain::new(source, prime, limit).err(), Some(Error::Limit));
        }
    }

    #[test]
    fn tail_progress_is_partial_and_eof_is_once() -> Result<(), Error> {
        let mut drain = Drain::new(5000, (2112, 0), 4096)?;
        drain.consume(0)?;
        assert_eq!(drain.remaining, 2168);
        drain.consume(120)?;
        assert_eq!(drain.remaining, 2048);
        assert_eq!(drain.mark_eof(), Err(Error::State));
        assert_eq!(drain.consume(2049), Err(Error::Output));
        assert_eq!(drain.remaining, 2048);
        drain.consume(1024)?;
        drain.consume(1024)?;
        drain.mark_eof()?;
        assert_eq!(drain.mark_eof(), Err(Error::State));
        assert_eq!(drain.consume(0), Err(Error::State));
        Ok(())
    }

    #[test]
    fn a_cancelled_tail_does_not_consume_padding_or_send_eof() -> Result<(), Error> {
        use std::time::{Duration, Instant};

        let control = Control::new(Instant::now() + Duration::from_secs(30));
        let limits = Limits {
            max_chunk_frames: 8192,
            max_total_frames: 32768,
            max_output_bytes: 65536,
            max_packets: 64,
            max_packet_bytes: 4096,
            max_cookie_bytes: 256,
            max_trim_frames: 4096,
        };
        let mut codec = Instance::new(Settings::new(44_100, 1, 128_000, 96)?, limits, &control)?;
        codec.eof()?;
        let before = codec.drain.ok_or(Error::State)?.remaining;
        control.cancel();
        assert_eq!(codec.produce(4096).err(), Some(Error::Cancelled));
        let after = codec.drain.ok_or(Error::State)?;
        assert_eq!(after.remaining, before);
        assert!(!after.eof);
        Ok(())
    }

    #[test]
    fn a_failed_factory_never_confers_ownership_of_its_output_pointer() {
        let nonnull = NonNull::<u8>::dangling().as_ptr().cast();
        assert_eq!(
            successful_handle(-1, nonnull),
            Err(Error::Framework {
                operation: Operation::Create,
                status: -1
            })
        );
        assert_eq!(
            successful_handle(-1, ptr::null_mut()),
            Err(Error::Framework {
                operation: Operation::Create,
                status: -1
            })
        );
        assert_eq!(successful_handle(0, ptr::null_mut()), Err(Error::Output));
        assert_eq!(
            successful_handle(0, nonnull).map(NonNull::as_ptr),
            Ok(nonnull)
        );
    }

    #[test]
    fn ffi_layouts_match_the_public_headers_on_supported_macos_targets() {
        assert_eq!(std::mem::size_of::<Format>(), 40);
        assert_eq!(std::mem::align_of::<Format>(), 8);
        assert_eq!(std::mem::size_of::<Description>(), 20);
        assert_eq!(std::mem::size_of::<Packet>(), 16);
        assert_eq!(std::mem::align_of::<Packet>(), 8);
        assert_eq!(std::mem::size_of::<Prime>(), 8);
    }

    #[test]
    fn a_cancellation_during_a_synchronous_call_is_observed_afterward() {
        let control = Control::new(std::time::Instant::now() + std::time::Duration::from_secs(30));
        assert_eq!(
            invoke(&control, Operation::Append, || {
                control.cancel();
                0
            }),
            Err(Error::Cancelled)
        );
        let mut called = false;
        assert_eq!(
            invoke(&control, Operation::Append, || {
                called = true;
                0
            }),
            Err(Error::Cancelled)
        );
        assert!(!called);
    }
}
