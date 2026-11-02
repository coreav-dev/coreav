#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use super::{Control, Error, Identity, Operation, RawOutput, Settings};
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
    pub(super) fn new(settings: Settings, control: &Control) -> Result<Self, Error> {
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
        if bytes > offered || bytes % (self.channels * 4) != 0 || frames > offered_frames {
            return Err(Error::Output);
        }
        usize::try_from(bytes / (self.channels * 4)).map_err(|_| Error::Output)
    }

    pub(super) fn eof(&mut self) -> Result<(), Error> {
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
        if bytes != 0 || packets != 0 {
            return Err(Error::Output);
        }
        Ok(())
    }

    pub(super) fn produce(&mut self, capacity: usize) -> Result<RawOutput, Error> {
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
        if bytes as usize > data.len() || count > 1 {
            return Err(Error::Output);
        }
        data.truncate(bytes as usize);
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

    pub(super) fn trim(&mut self) -> Result<(u32, u32), Error> {
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
        let trailing = prime
            .trailing
            .checked_add(self.u32(*b"pad0")?)
            .ok_or(Error::Output)?;
        Ok((prime.leading, trailing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
