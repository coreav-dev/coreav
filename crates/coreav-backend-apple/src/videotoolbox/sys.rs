#![allow(unsafe_code)]

use super::{
    Availability, Codec, Control, Error, Frame, Hardware, HardwarePolicy, Identity, Limits,
    Operation, RawSample, Settings, Time,
};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::rc::Rc;

type Ref = *const c_void;
type Callback = extern "C" fn(*mut c_void, *mut c_void, i32, u32, Ref);

const AVC1: u32 = u32::from_be_bytes(*b"avc1");
const HVC1: u32 = u32::from_be_bytes(*b"hvc1");
const VIDEO: u32 = u32::from_be_bytes(*b"vide");
const NV12: u32 = u32::from_be_bytes(*b"420v");
const FRAME_DROPPED: u32 = 1 << 1;
const PROPERTY_NOT_SUPPORTED: i32 = -12900;
const POOL_THRESHOLD: i32 = -6689;
const MAX_ENCODER_ID_BYTES: usize = 4096;
const MAX_ENCODER_LIST_ENTRIES: isize = 256;

#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    scale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    const INVALID: Self = Self {
        value: 0,
        scale: 0,
        flags: 0,
        epoch: 0,
    };

    fn numeric(time: Time) -> Self {
        Self {
            value: time.value,
            scale: time.scale,
            flags: 1,
            epoch: 0,
        }
    }

    fn time(self) -> Result<Time, Error> {
        if self.flags & !3 != 0 || self.flags & 1 == 0 || self.epoch != 0 {
            return Err(Error::Output);
        }
        Time::new(self.value, self.scale).map_err(|_| Error::Output)
    }
}

#[repr(C, packed(4))]
struct VideoDimensions {
    width: i32,
    height: i32,
}

#[repr(C)]
struct CFRange {
    location: isize,
    length: isize,
}

type Retain = Option<unsafe extern "C" fn(Ref, Ref) -> Ref>;
type Release = Option<unsafe extern "C" fn(Ref, Ref)>;
type Description = Option<unsafe extern "C" fn(Ref) -> Ref>;
type Equal = Option<unsafe extern "C" fn(Ref, Ref) -> u8>;

#[repr(C)]
struct DictionaryKeys {
    version: isize,
    retain: Retain,
    release: Release,
    description: Description,
    equal: Equal,
    hash: Option<unsafe extern "C" fn(Ref) -> usize>,
}

#[repr(C)]
struct DictionaryValues {
    version: isize,
    retain: Retain,
    release: Release,
    description: Description,
    equal: Equal,
}

#[cfg(test)]
#[repr(C)]
struct ArrayCallbacks {
    version: isize,
    retain: Retain,
    release: Release,
    description: Description,
    equal: Equal,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(value: Ref) -> Ref;
    fn CFRelease(value: Ref);
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFEqual(a: Ref, b: Ref) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(data: Ref) -> isize;
    fn CFDataGetBytePtr(data: Ref) -> *const u8;
    fn CFBooleanGetValue(value: Ref) -> u8;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberCreate(allocator: Ref, kind: isize, value: Ref) -> Ref;
    fn CFNumberGetValue(number: Ref, kind: isize, value: *mut c_void) -> u8;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        allocator: Ref,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> Ref;
    fn CFStringGetLength(string: Ref) -> isize;
    fn CFStringGetBytes(
        string: Ref,
        range: CFRange,
        encoding: u32,
        loss: u8,
        external: u8,
        buffer: *mut u8,
        capacity: isize,
        used: *mut isize,
    ) -> isize;
    fn CFDictionaryCreate(
        allocator: Ref,
        keys: *const Ref,
        values: *const Ref,
        count: isize,
        key_callbacks: *const DictionaryKeys,
        value_callbacks: *const DictionaryValues,
    ) -> Ref;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    #[cfg(test)]
    fn CFArrayCreate(
        allocator: Ref,
        values: *const Ref,
        count: isize,
        callbacks: *const ArrayCallbacks,
    ) -> Ref;
    static kCFTypeDictionaryKeyCallBacks: DictionaryKeys;
    static kCFTypeDictionaryValueCallBacks: DictionaryValues;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
    #[cfg(test)]
    static kCFTypeArrayCallBacks: ArrayCallbacks;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCompressionSessionCreate(
        allocator: Ref,
        width: i32,
        height: i32,
        codec: u32,
        specification: Ref,
        attributes: Ref,
        compressed_allocator: Ref,
        callback: Option<Callback>,
        context: *mut c_void,
        session: *mut Ref,
    ) -> i32;
    fn VTCompressionSessionGetTypeID() -> usize;
    fn VTCompressionSessionInvalidate(session: Ref);
    fn VTCompressionSessionPrepareToEncodeFrames(session: Ref) -> i32;
    fn VTCompressionSessionGetPixelBufferPool(session: Ref) -> Ref;
    fn VTCompressionSessionEncodeFrame(
        session: Ref,
        image: Ref,
        pts: CMTime,
        duration: CMTime,
        properties: Ref,
        frame_context: *mut c_void,
        flags: *mut u32,
    ) -> i32;
    fn VTCompressionSessionCompleteFrames(session: Ref, through: CMTime) -> i32;
    fn VTSessionSetProperty(session: Ref, key: Ref, value: Ref) -> i32;
    fn VTSessionCopyProperty(session: Ref, key: Ref, allocator: Ref, value: *mut c_void) -> i32;
    fn VTCopyVideoEncoderList(options: Ref, encoders: *mut Ref) -> i32;
    static kVTVideoEncoderList_EncoderID: Ref;
    static kVTVideoEncoderList_IsHardwareAccelerated: Ref;
    static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: Ref;
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: Ref;
    static kVTCompressionPropertyKey_AverageBitRate: Ref;
    static kVTCompressionPropertyKey_ProfileLevel: Ref;
    static kVTProfileLevel_H264_High_AutoLevel: Ref;
    static kVTProfileLevel_HEVC_Main_AutoLevel: Ref;
    static kVTCompressionPropertyKey_AllowFrameReordering: Ref;
    static kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: Ref;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: Ref;
    static kVTCompressionPropertyKey_ExpectedFrameRate: Ref;
    static kVTCompressionPropertyKey_RealTime: Ref;
    static kVTCompressionPropertyKey_ColorPrimaries: Ref;
    static kVTCompressionPropertyKey_TransferFunction: Ref;
    static kVTCompressionPropertyKey_YCbCrMatrix: Ref;
    static kVTCompressionPropertyKey_EncoderID: Ref;
    static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: Ref;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferPoolGetTypeID() -> usize;
    fn CVPixelBufferGetTypeID() -> usize;
    fn CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
        allocator: Ref,
        pool: Ref,
        attributes: Ref,
        buffer: *mut Ref,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(buffer: Ref, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(buffer: Ref, flags: u64) -> i32;
    fn CVPixelBufferGetWidth(buffer: Ref) -> usize;
    fn CVPixelBufferGetHeight(buffer: Ref) -> usize;
    fn CVPixelBufferGetPixelFormatType(buffer: Ref) -> u32;
    fn CVPixelBufferGetDataSize(buffer: Ref) -> usize;
    fn CVPixelBufferGetPlaneCount(buffer: Ref) -> usize;
    fn CVPixelBufferGetWidthOfPlane(buffer: Ref, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(buffer: Ref, plane: usize) -> usize;
    fn CVPixelBufferGetBytesPerRowOfPlane(buffer: Ref, plane: usize) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(buffer: Ref, plane: usize) -> *mut c_void;
    fn CVBufferSetAttachment(buffer: Ref, key: Ref, value: Ref, mode: u32);
    static kCVPixelBufferPixelFormatTypeKey: Ref;
    static kCVPixelBufferWidthKey: Ref;
    static kCVPixelBufferHeightKey: Ref;
    static kCVPixelBufferPoolAllocationThresholdKey: Ref;
    static kCVImageBufferColorPrimariesKey: Ref;
    static kCVImageBufferTransferFunctionKey: Ref;
    static kCVImageBufferYCbCrMatrixKey: Ref;
    static kCVImageBufferColorPrimaries_ITU_R_709_2: Ref;
    static kCVImageBufferTransferFunction_ITU_R_709_2: Ref;
    static kCVImageBufferYCbCrMatrix_ITU_R_709_2: Ref;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetTypeID() -> usize;
    fn CMSampleBufferDataIsReady(sample: Ref) -> u8;
    fn CMSampleBufferGetNumSamples(sample: Ref) -> isize;
    fn CMSampleBufferGetSampleSize(sample: Ref, index: isize) -> usize;
    fn CMSampleBufferGetDataBuffer(sample: Ref) -> Ref;
    fn CMSampleBufferGetFormatDescription(sample: Ref) -> Ref;
    fn CMSampleBufferGetPresentationTimeStamp(sample: Ref) -> CMTime;
    fn CMSampleBufferGetDuration(sample: Ref) -> CMTime;
    fn CMSampleBufferGetDecodeTimeStamp(sample: Ref) -> CMTime;
    fn CMSampleBufferGetSampleAttachmentsArray(sample: Ref, create: u8) -> Ref;
    fn CMBlockBufferGetTypeID() -> usize;
    fn CMBlockBufferGetDataLength(block: Ref) -> usize;
    fn CMBlockBufferCopyDataBytes(
        block: Ref,
        offset: usize,
        length: usize,
        destination: *mut c_void,
    ) -> i32;
    fn CMFormatDescriptionGetTypeID() -> usize;
    fn CMFormatDescriptionGetMediaType(format: Ref) -> u32;
    fn CMFormatDescriptionGetMediaSubType(format: Ref) -> u32;
    fn CMFormatDescriptionGetExtension(format: Ref, key: Ref) -> Ref;
    fn CMVideoFormatDescriptionGetDimensions(format: Ref) -> VideoDimensions;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        format: Ref,
        index: usize,
        bytes: *mut *const u8,
        length: *mut usize,
        count: *mut usize,
        nal_length: *mut i32,
    ) -> i32;
    fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(
        format: Ref,
        index: usize,
        bytes: *mut *const u8,
        length: *mut usize,
        count: *mut usize,
        nal_length: *mut i32,
    ) -> i32;
    static kCMSampleAttachmentKey_NotSync: Ref;
    static kCMFormatDescriptionExtension_FullRangeVideo: Ref;
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: Ref;
}

struct Owned(NonNull<c_void>);

impl Owned {
    /// # Safety
    /// `value` is a successful Create/Copy result, or a reference just retained
    /// by CFRetain. Failure outputs do not confer ownership.
    unsafe fn adopt(value: Ref) -> Result<Self, Error> {
        NonNull::new(value.cast_mut())
            .map(Self)
            .ok_or(Error::Output)
    }

    fn get(&self) -> Ref {
        self.0.as_ptr().cast_const()
    }

    /// # Safety
    /// `value` is a live CF object for the duration of CFRetain.
    unsafe fn retain(value: Ref) -> Result<Self, Error> {
        if value.is_null() {
            return Err(Error::Output);
        }
        unsafe { Self::adopt(CFRetain(value)) }
    }

    fn integer(value: i32) -> Result<Self, Error> {
        let number = unsafe { CFNumberCreate(ptr::null(), 3, ptr::from_ref(&value).cast()) };
        if number.is_null() {
            return Err(Error::Allocation);
        }
        unsafe { Self::adopt(number) }
    }

    fn real(value: f64) -> Result<Self, Error> {
        let number = unsafe { CFNumberCreate(ptr::null(), 6, ptr::from_ref(&value).cast()) };
        if number.is_null() {
            return Err(Error::Allocation);
        }
        unsafe { Self::adopt(number) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.get()) }
    }
}

/// # Safety
/// Every element is a live CF object. The standard CF callbacks retain the
/// keys and values before the stack arrays and caller references can expire.
unsafe fn dictionary<const N: usize>(keys: [Ref; N], values: [Ref; N]) -> Result<Owned, Error> {
    if keys
        .iter()
        .chain(values.iter())
        .any(|value| value.is_null())
    {
        return Err(Error::Output);
    }
    let count = isize::try_from(N).map_err(|_| Error::Limit)?;
    let result = unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            count,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    if result.is_null() {
        return Err(Error::Allocation);
    }
    unsafe { Owned::adopt(result) }
}

fn status(operation: Operation, code: i32) -> Result<(), Error> {
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Framework {
            operation,
            status: code,
        })
    }
}

fn creation_status(operation: Operation, code: i32) -> Result<(), Error> {
    match code {
        -12908 => Err(Error::Unavailable(Availability::NoEncoder)),
        -12915 => Err(Error::Unavailable(Availability::TemporarilyUnavailable)),
        _ => status(operation, code),
    }
}

fn controlled<T>(control: &Control, action: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    control.check()?;
    let result = action();
    control.check()?;
    result
}

#[derive(Clone, Copy)]
enum Expected {
    Integer(i32),
    Real(f64),
    Boolean(bool),
    String(Ref),
}

impl Expected {
    /// # Safety
    /// String values are live SDK CFString constants.
    unsafe fn owned(self) -> Result<Owned, Error> {
        match self {
            Self::Integer(value) => Owned::integer(value),
            Self::Real(value) => Owned::real(value),
            Self::Boolean(value) => unsafe {
                Owned::retain(if value {
                    kCFBooleanTrue
                } else {
                    kCFBooleanFalse
                })
            },
            Self::String(value) => unsafe { Owned::retain(value) },
        }
    }

    /// # Safety
    /// `actual` is a live CF object, and string expectations are SDK constants.
    unsafe fn matches(self, actual: Ref) -> bool {
        unsafe {
            match self {
                Self::Integer(expected) => {
                    let mut value = 0i32;
                    CFGetTypeID(actual) == CFNumberGetTypeID()
                        && CFNumberGetValue(actual, 3, ptr::from_mut(&mut value).cast()) != 0
                        && value == expected
                }
                Self::Real(expected) => {
                    let mut value = 0f64;
                    CFGetTypeID(actual) == CFNumberGetTypeID()
                        && CFNumberGetValue(actual, 6, ptr::from_mut(&mut value).cast()) != 0
                        && value.is_finite()
                        && value == expected
                }
                Self::Boolean(expected) => {
                    CFGetTypeID(actual) == CFBooleanGetTypeID()
                        && (CFBooleanGetValue(actual) != 0) == expected
                }
                Self::String(expected) => {
                    CFGetTypeID(actual) == CFStringGetTypeID() && CFEqual(actual, expected) != 0
                }
            }
        }
    }
}

pub(super) struct Native {
    key: usize,
    session: Option<Owned>,
    pool: Option<Owned>,
    allocation: Option<Owned>,
    settings: Settings,
    limits: Limits,
    control: Control,
    owner_thread: PhantomData<Rc<()>>,
}

impl Native {
    #[cfg(test)]
    pub(super) fn without_session(
        key: usize,
        settings: Settings,
        limits: Limits,
        control: Control,
    ) -> Self {
        Self {
            key,
            session: None,
            pool: None,
            allocation: None,
            settings,
            limits,
            control,
            owner_thread: PhantomData,
        }
    }

    pub(super) fn new(
        settings: Settings,
        key: usize,
        limits: Limits,
        control: Control,
    ) -> Result<Self, Error> {
        control.check()?;
        if key == 0
            || settings.rate_den == 0
            || settings.rate_num == 0
            || limits.max_pool_buffers == 0
        {
            return Err(Error::Input);
        }
        let width = i32::try_from(settings.dimensions.width()).map_err(|_| Error::Input)?;
        let height = i32::try_from(settings.dimensions.height()).map_err(|_| Error::Input)?;
        let pixels = u64::from(settings.dimensions.width())
            .checked_mul(u64::from(settings.dimensions.height()))
            .and_then(|value| value.checked_add(value / 2))
            .ok_or(Error::Limit)?;
        if settings.dimensions.width() > limits.max_width
            || settings.dimensions.height() > limits.max_height
            || usize::try_from(pixels).map_err(|_| Error::Limit)? > limits.max_pixel_buffer_bytes
        {
            return Err(Error::Limit);
        }
        let format = controlled(&control, || {
            Owned::integer(i32::try_from(NV12).map_err(|_| Error::Input)?)
        })?;
        let w = controlled(&control, || Owned::integer(width))?;
        let h = controlled(&control, || Owned::integer(height))?;
        let threshold = i32::try_from(limits.max_pool_buffers).map_err(|_| Error::Limit)?;
        let threshold = controlled(&control, || Owned::integer(threshold))?;
        let allocation = controlled(&control, || unsafe {
            dictionary(
                [kCVPixelBufferPoolAllocationThresholdKey],
                [threshold.get()],
            )
        })?;
        let attributes = controlled(&control, || unsafe {
            dictionary(
                [
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferWidthKey,
                    kCVPixelBufferHeightKey,
                ],
                [format.get(), w.get(), h.get()],
            )
        })?;
        let specification = controlled(&control, || unsafe {
            let (key, value) = match settings.hardware {
                HardwarePolicy::Any => (
                    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
                    kCFBooleanTrue,
                ),
                HardwarePolicy::Require => (
                    kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
                    kCFBooleanTrue,
                ),
                HardwarePolicy::Disable => (
                    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
                    kCFBooleanFalse,
                ),
            };
            dictionary([key], [value])
        })?;
        let mut session = ptr::null();
        control.check()?;
        let code = unsafe {
            VTCompressionSessionCreate(
                ptr::null(),
                width,
                height,
                match settings.codec {
                    Codec::H264 => AVC1,
                    Codec::Hevc => HVC1,
                },
                specification.get(),
                attributes.get(),
                ptr::null(),
                Some(output_callback),
                ptr::without_provenance_mut(key),
                &mut session,
            )
        };
        if let Err(error) = creation_status(Operation::Create, code) {
            control.check()?;
            return Err(error);
        }
        let session_owner = unsafe { Owned::adopt(session)? };
        if unsafe { CFGetTypeID(session) != VTCompressionSessionGetTypeID() } {
            return Err(Error::Output);
        }
        let mut native = Self {
            key,
            session: Some(session_owner),
            pool: None,
            allocation: Some(allocation),
            settings,
            limits,
            control,
            owner_thread: PhantomData,
        };
        native.control.check()?;
        native.configure()?;
        native.pool = Some(controlled(&native.control, || unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(session);
            if pool.is_null() || CFGetTypeID(pool) != CVPixelBufferPoolGetTypeID() {
                return Err(Error::Output);
            }
            Owned::retain(pool)
        })?);
        native.identity()?;
        Ok(native)
    }

    fn session(&self) -> Result<Ref, Error> {
        self.session.as_ref().map(Owned::get).ok_or(Error::State)
    }

    fn configure(&self) -> Result<(), Error> {
        self.control.check()?;
        let session = self.session()?;
        let settings = self.settings;
        let bitrate = i32::try_from(settings.bitrate).map_err(|_| Error::Input)?;
        let interval = u64::from(settings.rate_num) * 2 / u64::from(settings.rate_den);
        let interval = i32::try_from(interval).map_err(|_| Error::Input)?;
        let cadence_hint =
            i32::try_from(settings.expected_frame_rate_hint()).map_err(|_| Error::Input)?;
        if bitrate <= 0 || interval <= 0 {
            return Err(Error::Input);
        }
        let controls = unsafe {
            [
                (
                    kVTCompressionPropertyKey_AverageBitRate,
                    Expected::Integer(bitrate),
                    "AverageBitRate",
                ),
                (
                    kVTCompressionPropertyKey_ProfileLevel,
                    Expected::String(match settings.codec {
                        Codec::H264 => kVTProfileLevel_H264_High_AutoLevel,
                        Codec::Hevc => kVTProfileLevel_HEVC_Main_AutoLevel,
                    }),
                    "ProfileLevel",
                ),
                (
                    kVTCompressionPropertyKey_AllowFrameReordering,
                    Expected::Boolean(false),
                    "AllowFrameReordering",
                ),
                (
                    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                    Expected::Real(2.0),
                    "MaxKeyFrameIntervalDuration",
                ),
                (
                    kVTCompressionPropertyKey_MaxKeyFrameInterval,
                    Expected::Integer(interval),
                    "MaxKeyFrameInterval",
                ),
                (
                    kVTCompressionPropertyKey_ExpectedFrameRate,
                    Expected::Integer(cadence_hint),
                    "ExpectedFrameRate",
                ),
                (
                    kVTCompressionPropertyKey_RealTime,
                    Expected::Boolean(false),
                    "RealTime",
                ),
                (
                    kVTCompressionPropertyKey_ColorPrimaries,
                    Expected::String(kCVImageBufferColorPrimaries_ITU_R_709_2),
                    "ColorPrimaries",
                ),
                (
                    kVTCompressionPropertyKey_TransferFunction,
                    Expected::String(kCVImageBufferTransferFunction_ITU_R_709_2),
                    "TransferFunction",
                ),
                (
                    kVTCompressionPropertyKey_YCbCrMatrix,
                    Expected::String(kCVImageBufferYCbCrMatrix_ITU_R_709_2),
                    "YCbCrMatrix",
                ),
            ]
        };
        for (key, expected, _) in controls {
            let value = controlled(&self.control, || unsafe { expected.owned() })?;
            controlled(&self.control, || {
                status(Operation::Configure, unsafe {
                    VTSessionSetProperty(session, key, value.get())
                })
            })?;
        }
        controlled(&self.control, || {
            creation_status(Operation::Prepare, unsafe {
                VTCompressionSessionPrepareToEncodeFrames(session)
            })
        })?;
        for (key, expected, name) in controls {
            let value = self.property(key)?.ok_or(Error::Setting(name))?;
            if !controlled(&self.control, || {
                Ok(unsafe { expected.matches(value.get()) })
            })? {
                return Err(Error::Setting(name));
            }
        }
        Ok(())
    }

    fn property(&self, key: Ref) -> Result<Option<Owned>, Error> {
        controlled(&self.control, || {
            let mut value: Ref = ptr::null();
            let code = unsafe {
                VTSessionCopyProperty(
                    self.session()?,
                    key,
                    ptr::null(),
                    ptr::from_mut(&mut value).cast(),
                )
            };
            if code == PROPERTY_NOT_SUPPORTED {
                return Ok(None);
            }
            status(Operation::Query, code)?;
            if value.is_null() {
                return Ok(None);
            }
            unsafe { Owned::adopt(value).map(Some) }
        })
    }

    pub(super) fn identity(&self) -> Result<Identity, Error> {
        self.control.check()?;
        let identifier = self.property(unsafe { kVTCompressionPropertyKey_EncoderID })?;
        let encoder_id = match identifier.as_ref() {
            Some(value) => Some(unsafe { encoder_id(value.get(), &self.control)? }),
            None => None,
        };
        let hardware = match self
            .property(unsafe { kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder })?
        {
            Some(value) => controlled(&self.control, || unsafe {
                if CFGetTypeID(value.get()) != CFBooleanGetTypeID() {
                    return Err(Error::Output);
                }
                if CFBooleanGetValue(value.get()) != 0 {
                    Ok(Hardware::Hardware)
                } else {
                    Ok(Hardware::Software)
                }
            })?,
            None => {
                let identifier = identifier.as_ref().ok_or(Error::UnknownHardware)?;
                let list = controlled(&self.control, || unsafe {
                    let mut list = ptr::null();
                    status(
                        Operation::Query,
                        VTCopyVideoEncoderList(ptr::null(), &mut list),
                    )?;
                    Owned::adopt(list)
                })?;
                controlled(&self.control, || unsafe {
                    hardware_from_list(identifier.get(), list.get(), &self.control)
                })?
            }
        };
        match (self.settings.hardware, hardware) {
            (HardwarePolicy::Require | HardwarePolicy::Disable, Hardware::Unknown) => {
                return Err(Error::UnknownHardware);
            }
            (HardwarePolicy::Require, Hardware::Software)
            | (HardwarePolicy::Disable, Hardware::Hardware) => {
                return Err(Error::Setting("HardwarePolicy"));
            }
            _ => {}
        }
        self.control.check()?;
        Ok(Identity {
            encoder_id,
            hardware,
        })
    }

    pub(super) fn submit(
        &mut self,
        frame: Frame<'_>,
        frame_key: usize,
        pts: Time,
        duration: Time,
    ) -> Result<(), Error> {
        self.control.check()?;
        let session = self.session()?;
        if frame_key == 0 {
            return Err(Error::Input);
        }
        let pool = self.pool.as_ref().ok_or(Error::State)?;
        let allocation = self.allocation.as_ref().ok_or(Error::State)?;
        let buffer = controlled(&self.control, || unsafe {
            let mut buffer = ptr::null();
            let code = CVPixelBufferPoolCreatePixelBufferWithAuxAttributes(
                ptr::null(),
                pool.get(),
                allocation.get(),
                &mut buffer,
            );
            if code == POOL_THRESHOLD {
                return Err(Error::Limit);
            }
            status(Operation::PixelBuffer, code)?;
            let buffer = Owned::adopt(buffer)?;
            if CFGetTypeID(buffer.get()) != CVPixelBufferGetTypeID() {
                return Err(Error::Output);
            }
            Ok(buffer)
        })?;
        let mut lock = controlled(&self.control, || PixelLock::new(&buffer))?;
        let copied = unsafe {
            copy_frame(
                buffer.get(),
                frame,
                self.settings,
                self.limits,
                &self.control,
            )
        };
        let unlocked = lock.unlock();
        self.control.check()?;
        unlocked?;
        copied?;
        for (key, value) in unsafe {
            [
                (
                    kCVImageBufferColorPrimariesKey,
                    kCVImageBufferColorPrimaries_ITU_R_709_2,
                ),
                (
                    kCVImageBufferTransferFunctionKey,
                    kCVImageBufferTransferFunction_ITU_R_709_2,
                ),
                (
                    kCVImageBufferYCbCrMatrixKey,
                    kCVImageBufferYCbCrMatrix_ITU_R_709_2,
                ),
            ]
        } {
            controlled(&self.control, || {
                unsafe { CVBufferSetAttachment(buffer.get(), key, value, 1) };
                Ok(())
            })?;
        }
        let mut flags = 0;
        controlled(&self.control, || {
            status(Operation::Encode, unsafe {
                VTCompressionSessionEncodeFrame(
                    session,
                    buffer.get(),
                    CMTime::numeric(pts),
                    CMTime::numeric(duration),
                    ptr::null(),
                    ptr::without_provenance_mut(frame_key),
                    &mut flags,
                )
            })
        })?;
        if flags & FRAME_DROPPED != 0 {
            return Err(Error::Output);
        }
        Ok(())
    }

    pub(super) fn complete(&mut self, through: Option<Time>) -> Result<(), Error> {
        controlled(&self.control, || {
            status(Operation::Complete, unsafe {
                VTCompressionSessionCompleteFrames(
                    self.session()?,
                    through.map_or(CMTime::INVALID, CMTime::numeric),
                )
            })
        })
    }

    pub(super) fn close(&mut self) {
        super::retire_callback(self.key);
        if let Some(session) = self.session.take() {
            unsafe { VTCompressionSessionInvalidate(session.get()) }
            drop(session);
        }
        self.pool.take();
        self.allocation.take();
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        self.close();
    }
}

struct PixelLock<'a> {
    buffer: &'a Owned,
    locked: bool,
}

impl<'a> PixelLock<'a> {
    fn new(buffer: &'a Owned) -> Result<Self, Error> {
        status(Operation::Lock, unsafe {
            CVPixelBufferLockBaseAddress(buffer.get(), 0)
        })?;
        Ok(Self {
            buffer,
            locked: true,
        })
    }

    fn unlock(&mut self) -> Result<(), Error> {
        if !self.locked {
            return Ok(());
        }
        self.locked = false;
        status(Operation::Unlock, unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer.get(), 0)
        })
    }
}

impl Drop for PixelLock<'_> {
    fn drop(&mut self) {
        let _ = self.unlock();
    }
}

/// # Safety
/// `buffer` is an exclusively held pool buffer with a successful write lock.
/// CoreVideo owns each plane's stride-by-height storage until unlock. The
/// returned plane addresses are checked before any temporary slice is made.
unsafe fn copy_frame(
    buffer: Ref,
    frame: Frame<'_>,
    settings: Settings,
    limits: Limits,
    control: &Control,
) -> Result<(), Error> {
    control.check()?;
    let width = usize::try_from(settings.dimensions.width()).map_err(|_| Error::Limit)?;
    let height = usize::try_from(settings.dimensions.height()).map_err(|_| Error::Limit)?;
    let source_len = width.checked_mul(height).ok_or(Error::Limit)?;
    if frame.dimensions.width() != settings.dimensions.width()
        || frame.dimensions.height() != settings.dimensions.height()
        || frame.y.len() != source_len
        || frame.u.len() != source_len / 4
        || frame.v.len() != source_len / 4
    {
        return Err(Error::Input);
    }
    unsafe {
        control.check()?;
        if CVPixelBufferGetPixelFormatType(buffer) != NV12
            || CVPixelBufferGetWidth(buffer) != width
            || CVPixelBufferGetHeight(buffer) != height
            || CVPixelBufferGetPlaneCount(buffer) != 2
        {
            return Err(Error::Output);
        }
        let data_size = CVPixelBufferGetDataSize(buffer);
        control.check()?;
        if data_size == 0
            || data_size > limits.max_pixel_buffer_bytes
            || data_size > isize::MAX as usize
        {
            return Err(Error::Limit);
        }
        let mut planes = [(ptr::null_mut::<u8>(), 0usize, 0usize); 2];
        let mut total = 0usize;
        for (index, plane) in planes.iter_mut().enumerate() {
            control.check()?;
            let plane_height = if index == 0 { height } else { height / 2 };
            let plane_width = if index == 0 { width } else { width / 2 };
            if CVPixelBufferGetWidthOfPlane(buffer, index) != plane_width
                || CVPixelBufferGetHeightOfPlane(buffer, index) != plane_height
            {
                return Err(Error::Output);
            }
            let stride = CVPixelBufferGetBytesPerRowOfPlane(buffer, index);
            let length = stride.checked_mul(plane_height).ok_or(Error::Limit)?;
            let address = CVPixelBufferGetBaseAddressOfPlane(buffer, index).cast::<u8>();
            control.check()?;
            if stride < width
                || address.is_null()
                || length == 0
                || length > isize::MAX as usize
                || address.addr().checked_add(length).is_none()
            {
                return Err(Error::Output);
            }
            total = total.checked_add(length).ok_or(Error::Limit)?;
            *plane = (address, stride, length);
        }
        if total > data_size || total > limits.max_pixel_buffer_bytes {
            return Err(Error::Limit);
        }
        let [(y, y_stride, y_len), (uv, uv_stride, uv_len)] = planes;
        let y_end = y.addr().checked_add(y_len).ok_or(Error::Output)?;
        let uv_end = uv.addr().checked_add(uv_len).ok_or(Error::Output)?;
        if y.addr() < uv_end && uv.addr() < y_end {
            return Err(Error::Output);
        }
        y.write_bytes(0, y_len);
        let y_plane = std::slice::from_raw_parts_mut(y, y_len);
        for (destination, source) in y_plane
            .chunks_exact_mut(y_stride)
            .zip(frame.y.chunks_exact(width))
        {
            control.check()?;
            destination
                .get_mut(..width)
                .ok_or(Error::Output)?
                .copy_from_slice(source);
        }
        uv.write_bytes(0, uv_len);
        let uv_plane = std::slice::from_raw_parts_mut(uv, uv_len);
        for ((destination, u), v) in uv_plane
            .chunks_exact_mut(uv_stride)
            .zip(frame.u.chunks_exact(width / 2))
            .zip(frame.v.chunks_exact(width / 2))
        {
            control.check()?;
            let row = destination.get_mut(..width).ok_or(Error::Output)?;
            for ((pair, u), v) in row.chunks_exact_mut(2).zip(u).zip(v) {
                let [out_u, out_v] = pair else {
                    return Err(Error::Output);
                };
                *out_u = *u;
                *out_v = *v;
            }
        }
    }
    control.check()
}

fn initialized_bytes(length: usize) -> Result<Vec<u8>, Error> {
    if length > isize::MAX as usize {
        return Err(Error::Limit);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Error::Allocation)?;
    bytes.resize(length, 0);
    Ok(bytes)
}

pub(super) fn encoder_hardware(identifier: &str, control: &Control) -> Result<Hardware, Error> {
    let identifier = controlled(control, || unsafe {
        let value = CFStringCreateWithBytes(
            ptr::null(),
            identifier.as_ptr(),
            isize::try_from(identifier.len()).map_err(|_| Error::Limit)?,
            0x08000100,
            0,
        );
        if value.is_null() {
            return Err(Error::Allocation);
        }
        Owned::adopt(value)
    })?;
    let list = controlled(control, || unsafe {
        let mut list = ptr::null();
        status(
            Operation::Query,
            VTCopyVideoEncoderList(ptr::null(), &mut list),
        )?;
        Owned::adopt(list)
    })?;
    controlled(control, || unsafe {
        hardware_from_list(identifier.get(), list.get(), control)
    })
}

/// # Safety
/// `identifier` and `list` are live CF objects for the entire call.
unsafe fn hardware_from_list(
    identifier: Ref,
    list: Ref,
    control: &Control,
) -> Result<Hardware, Error> {
    enum MatchedHardware {
        MissingFlag,
        Flag(bool),
    }
    control.check()?;
    unsafe {
        if identifier.is_null()
            || list.is_null()
            || CFGetTypeID(identifier) != CFStringGetTypeID()
            || CFGetTypeID(list) != CFArrayGetTypeID()
        {
            return Err(Error::Output);
        }
        let length = CFStringGetLength(identifier);
        if length <= 0 || usize::try_from(length).map_err(|_| Error::Limit)? > MAX_ENCODER_ID_BYTES
        {
            return Err(Error::Limit);
        }
        let count = CFArrayGetCount(list);
        if !(0..=MAX_ENCODER_LIST_ENTRIES).contains(&count) {
            return Err(Error::Limit);
        }
        let mut hardware = None;
        for index in 0..count {
            control.check()?;
            let entry = CFArrayGetValueAtIndex(list, index);
            if entry.is_null() || CFGetTypeID(entry) != CFDictionaryGetTypeID() {
                return Err(Error::Output);
            }
            let candidate = CFDictionaryGetValue(entry, kVTVideoEncoderList_EncoderID);
            if candidate.is_null() || CFGetTypeID(candidate) != CFStringGetTypeID() {
                return Err(Error::Output);
            }
            let length = CFStringGetLength(candidate);
            if length <= 0
                || usize::try_from(length).map_err(|_| Error::Limit)? > MAX_ENCODER_ID_BYTES
            {
                return Err(Error::Limit);
            }
            if CFEqual(candidate, identifier) == 0 {
                continue;
            }
            if hardware.is_some() {
                return Err(Error::UnknownHardware);
            }
            let flag = CFDictionaryGetValue(entry, kVTVideoEncoderList_IsHardwareAccelerated);
            if flag.is_null() {
                hardware = Some(MatchedHardware::MissingFlag);
                continue;
            }
            if CFGetTypeID(flag) != CFBooleanGetTypeID() {
                return Err(Error::UnknownHardware);
            }
            hardware = Some(MatchedHardware::Flag(CFBooleanGetValue(flag) != 0));
        }
        control.check()?;
        match hardware {
            Some(MatchedHardware::Flag(true)) => Ok(Hardware::Hardware),
            Some(MatchedHardware::Flag(false)) => Ok(Hardware::Software),
            Some(MatchedHardware::MissingFlag) => Err(Error::MissingHardwareFlag),
            None => Err(Error::UnknownHardware),
        }
    }
}

/// # Safety
/// `value` is a live CF object from a successful copied session property.
unsafe fn encoder_id(value: Ref, control: &Control) -> Result<String, Error> {
    control.check()?;
    unsafe {
        if CFGetTypeID(value) != CFStringGetTypeID() {
            return Err(Error::Output);
        }
        let length = CFStringGetLength(value);
        control.check()?;
        let count = usize::try_from(length).map_err(|_| Error::Output)?;
        if count == 0 || count > MAX_ENCODER_ID_BYTES {
            return Err(Error::Limit);
        }
        let capacity = count
            .checked_mul(3)
            .ok_or(Error::Limit)?
            .min(MAX_ENCODER_ID_BYTES);
        let mut bytes = initialized_bytes(capacity)?;
        let mut used = 0isize;
        control.check()?;
        let converted = CFStringGetBytes(
            value,
            CFRange {
                location: 0,
                length,
            },
            0x08000100,
            0,
            0,
            bytes.as_mut_ptr(),
            isize::try_from(capacity).map_err(|_| Error::Limit)?,
            &mut used,
        );
        control.check()?;
        let used = usize::try_from(used).map_err(|_| Error::Output)?;
        if converted != length || used == 0 || used > bytes.len() {
            return Err(Error::Output);
        }
        bytes.truncate(used);
        String::from_utf8(bytes).map_err(|_| Error::Output)
    }
}

extern "C" fn output_callback(
    context: *mut c_void,
    frame_context: *mut c_void,
    code: i32,
    flags: u32,
    sample: Ref,
) {
    let session_key = context.addr();
    let frame_key = frame_context.addr();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(lease) = super::lease_callback(session_key, frame_key) else {
            return;
        };
        let result = if code != 0 {
            Err(Error::Framework {
                operation: Operation::Callback,
                status: code,
            })
        } else if flags & FRAME_DROPPED != 0 || sample.is_null() {
            Err(Error::Output)
        } else {
            unsafe { extract(sample, lease.settings(), lease.limits(), &lease.control()) }
        };
        lease.finish(result);
    }));
    if let Err(payload) = result {
        std::mem::forget(payload);
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| super::callback_failed(session_key)))
        {
            std::mem::forget(payload);
        }
    }
}

/// # Safety
/// `sample` is the live non-null sample supplied by Apple's successful callback.
/// No borrowed native reference or slice survives this callback invocation.
unsafe fn extract(
    sample: Ref,
    settings: Settings,
    limits: Limits,
    control: &Control,
) -> Result<RawSample, Error> {
    control.check()?;
    unsafe {
        if CFGetTypeID(sample) != CMSampleBufferGetTypeID()
            || CMSampleBufferDataIsReady(sample) == 0
            || CMSampleBufferGetNumSamples(sample) != 1
        {
            return Err(Error::Output);
        }
        control.check()?;
        let block = CMSampleBufferGetDataBuffer(sample);
        if block.is_null() || CFGetTypeID(block) != CMBlockBufferGetTypeID() {
            return Err(Error::Output);
        }
        let size = CMSampleBufferGetSampleSize(sample, 0);
        if size == 0 || size != CMBlockBufferGetDataLength(block) {
            return Err(Error::Output);
        }
        control.check()?;
        if size > limits.max_access_unit_bytes {
            return Err(Error::Limit);
        }
        let format = CMSampleBufferGetFormatDescription(sample);
        if format.is_null() || CFGetTypeID(format) != CMFormatDescriptionGetTypeID() {
            return Err(Error::Output);
        }
        let format = controlled(control, || Owned::retain(format))?;
        if CMFormatDescriptionGetMediaType(format.get()) != VIDEO
            || CMFormatDescriptionGetMediaSubType(format.get())
                != match settings.codec {
                    Codec::H264 => AVC1,
                    Codec::Hevc => HVC1,
                }
        {
            return Err(Error::Output);
        }
        control.check()?;
        for (key, expected) in [
            (
                kCVImageBufferColorPrimariesKey,
                kCVImageBufferColorPrimaries_ITU_R_709_2,
            ),
            (
                kCVImageBufferTransferFunctionKey,
                kCVImageBufferTransferFunction_ITU_R_709_2,
            ),
            (
                kCVImageBufferYCbCrMatrixKey,
                kCVImageBufferYCbCrMatrix_ITU_R_709_2,
            ),
        ] {
            control.check()?;
            let value = CMFormatDescriptionGetExtension(format.get(), key);
            if value.is_null() || !Expected::String(expected).matches(value) {
                return Err(Error::Output);
            }
            control.check()?;
        }
        let full_range = CMFormatDescriptionGetExtension(
            format.get(),
            kCMFormatDescriptionExtension_FullRangeVideo,
        );
        if !full_range.is_null() && !Expected::Boolean(false).matches(full_range) {
            return Err(Error::Output);
        }
        control.check()?;
        let dimensions = CMVideoFormatDescriptionGetDimensions(format.get());
        control.check()?;
        let width = u32::try_from(dimensions.width).map_err(|_| Error::Output)?;
        let height = u32::try_from(dimensions.height).map_err(|_| Error::Output)?;
        if width != settings.dimensions.width() || height != settings.dimensions.height() {
            return Err(Error::Output);
        }
        if width > limits.max_width || height > limits.max_height {
            return Err(Error::Limit);
        }
        let mut count = 0;
        let mut nal_length = 0;
        let get_parameter_set = match settings.codec {
            Codec::H264 => CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
            Codec::Hevc => CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
        };
        let expected_count = match settings.codec {
            Codec::H264 => 2,
            Codec::Hevc => 3,
        };
        controlled(control, || {
            status(
                Operation::Format,
                get_parameter_set(
                    format.get(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut count,
                    &mut nal_length,
                ),
            )
        })?;
        if count != expected_count || nal_length != 4 {
            return Err(Error::Output);
        }
        let mut sets = [(ptr::null::<u8>(), 0usize); 3];
        let mut total = 15usize;
        for (index, (pointer, length)) in sets.iter_mut().take(expected_count).enumerate() {
            controlled(control, || {
                status(
                    Operation::Format,
                    get_parameter_set(
                        format.get(),
                        index,
                        pointer,
                        length,
                        &mut count,
                        &mut nal_length,
                    ),
                )
            })?;
            if count != expected_count
                || nal_length != 4
                || pointer.is_null()
                || *length == 0
                || *length > isize::MAX as usize
                || pointer.addr().checked_add(*length).is_none()
            {
                return Err(Error::Output);
            }
            total = total.checked_add(*length).ok_or(Error::Limit)?;
        }
        if total > limits.max_configuration_bytes {
            return Err(Error::Limit);
        }
        let hvcc = if settings.codec == Codec::Hevc {
            let atoms = CMFormatDescriptionGetExtension(
                format.get(),
                kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms,
            );
            if atoms.is_null() || CFGetTypeID(atoms) != CFDictionaryGetTypeID() {
                return Err(Error::Output);
            }
            let key = controlled(control, || {
                Owned::adopt(CFStringCreateWithBytes(
                    ptr::null(),
                    b"hvcC".as_ptr(),
                    4,
                    0x08000100,
                    0,
                ))
            })?;
            let data = CFDictionaryGetValue(atoms, key.get());
            if data.is_null() || CFGetTypeID(data) != CFDataGetTypeID() {
                return Err(Error::Output);
            }
            let length = usize::try_from(CFDataGetLength(data)).map_err(|_| Error::Output)?;
            if length == 0
                || total
                    .checked_add(length)
                    .is_none_or(|n| n > limits.max_configuration_bytes)
            {
                return Err(Error::Limit);
            }
            let source = CFDataGetBytePtr(data);
            if source.is_null()
                || length > isize::MAX as usize
                || source.addr().checked_add(length).is_none()
            {
                return Err(Error::Output);
            }
            let mut bytes = initialized_bytes(length)?;
            control.check()?;
            bytes.copy_from_slice(std::slice::from_raw_parts(source, length));
            bytes
        } else {
            Vec::new()
        };
        let pts = CMSampleBufferGetPresentationTimeStamp(sample).time()?;
        let duration = CMSampleBufferGetDuration(sample).time()?;
        if duration.value <= 0 {
            return Err(Error::Output);
        }
        let native_dts = CMSampleBufferGetDecodeTimeStamp(sample);
        control.check()?;
        let dts = if native_dts.flags & 1 == 0 {
            None
        } else {
            Some(native_dts.time()?)
        };
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
        control.check()?;
        let sync = if attachments.is_null() {
            true
        } else {
            if CFGetTypeID(attachments) != CFArrayGetTypeID() || CFArrayGetCount(attachments) != 1 {
                return Err(Error::Output);
            }
            let dictionary = CFArrayGetValueAtIndex(attachments, 0);
            if dictionary.is_null() || CFGetTypeID(dictionary) != CFDictionaryGetTypeID() {
                return Err(Error::Output);
            }
            let not_sync = CFDictionaryGetValue(dictionary, kCMSampleAttachmentKey_NotSync);
            if not_sync.is_null() {
                true
            } else {
                if CFGetTypeID(not_sync) != CFBooleanGetTypeID() {
                    return Err(Error::Output);
                }
                CFBooleanGetValue(not_sync) == 0
            }
        };
        control.check()?;
        let mut bytes = initialized_bytes(size)?;
        controlled(control, || {
            status(
                Operation::BlockCopy,
                CMBlockBufferCopyDataBytes(block, 0, size, bytes.as_mut_ptr().cast()),
            )
        })?;
        let [(first, first_len), (second, second_len), (third, third_len)] = sets;
        let (vps, vps_len, sps, sps_len, pps, pps_len) = match settings.codec {
            Codec::H264 => (ptr::null(), 0, first, first_len, second, second_len),
            Codec::Hevc => (first, first_len, second, second_len, third, third_len),
        };
        let mut vps_bytes = initialized_bytes(vps_len)?;
        if vps_len != 0 {
            control.check()?;
            vps_bytes.copy_from_slice(std::slice::from_raw_parts(vps, vps_len));
        }
        let mut sps_bytes = initialized_bytes(sps_len)?;
        control.check()?;
        sps_bytes.copy_from_slice(std::slice::from_raw_parts(sps, sps_len));
        let mut pps_bytes = initialized_bytes(pps_len)?;
        control.check()?;
        pps_bytes.copy_from_slice(std::slice::from_raw_parts(pps, pps_len));
        control.check()?;
        Ok(RawSample {
            bytes,
            vps: vps_bytes,
            sps: sps_bytes,
            pps: pps_bytes,
            hvcc,
            dimensions: (width, height),
            pts,
            duration,
            dts,
            sync,
        })
    }
}

#[cfg(test)]
mod identity_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::time::{Duration, Instant};

    fn array(values: &[Ref]) -> Owned {
        unsafe {
            Owned::adopt(CFArrayCreate(
                ptr::null(),
                values.as_ptr(),
                isize::try_from(values.len()).unwrap(),
                &kCFTypeArrayCallBacks,
            ))
            .unwrap()
        }
    }

    fn entry(identifier: Ref, flag: Ref) -> Owned {
        unsafe {
            dictionary(
                [
                    kVTVideoEncoderList_EncoderID,
                    kVTVideoEncoderList_IsHardwareAccelerated,
                ],
                [identifier, flag],
            )
            .unwrap()
        }
    }

    #[test]
    fn encoder_list_hardware_requires_one_exact_id_and_explicit_boolean() {
        let control = Control::new(Instant::now() + Duration::from_secs(60));
        let identifier = unsafe { kVTProfileLevel_H264_High_AutoLevel };
        let other = unsafe { kCVImageBufferColorPrimaries_ITU_R_709_2 };
        let software = entry(identifier, unsafe { kCFBooleanFalse });
        let hardware = entry(identifier, unsafe { kCFBooleanTrue });
        let unrelated = entry(other, unsafe { kCFBooleanTrue });
        for (entry, expected) in [
            (&software, Hardware::Software),
            (&hardware, Hardware::Hardware),
        ] {
            let list = array(&[unrelated.get(), entry.get()]);
            assert_eq!(
                unsafe { hardware_from_list(identifier, list.get(), &control) },
                Ok(expected)
            );
        }
        let number = Owned::integer(0).unwrap();
        let nonboolean = entry(identifier, number.get());
        let missing_flag =
            unsafe { dictionary([kVTVideoEncoderList_EncoderID], [identifier]) }.unwrap();
        for values in [
            vec![],
            vec![unrelated.get()],
            vec![software.get(), software.get()],
            vec![software.get(), hardware.get()],
            vec![nonboolean.get()],
            vec![missing_flag.get(), software.get()],
            vec![software.get(), missing_flag.get()],
            vec![missing_flag.get(), missing_flag.get()],
        ] {
            let list = array(&values);
            assert_eq!(
                unsafe { hardware_from_list(identifier, list.get(), &control) },
                Err(Error::UnknownHardware)
            );
        }
        let list = array(&[unrelated.get(), missing_flag.get()]);
        assert_eq!(
            unsafe { hardware_from_list(identifier, list.get(), &control) },
            Err(Error::MissingHardwareFlag)
        );
        let invalid_id = entry(number.get(), unsafe { kCFBooleanTrue });
        let missing_id = unsafe {
            dictionary(
                [kVTVideoEncoderList_IsHardwareAccelerated],
                [kCFBooleanTrue],
            )
        }
        .unwrap();
        for values in [
            vec![number.get()],
            vec![invalid_id.get()],
            vec![missing_id.get()],
            vec![missing_flag.get(), missing_id.get()],
            vec![missing_flag.get(), number.get()],
        ] {
            let list = array(&values);
            assert_eq!(
                unsafe { hardware_from_list(identifier, list.get(), &control) },
                Err(Error::Output)
            );
        }
        assert_eq!(
            unsafe { hardware_from_list(identifier, number.get(), &control) },
            Err(Error::Output)
        );
        let list = array(&vec![software.get(); 257]);
        assert_eq!(
            unsafe { hardware_from_list(identifier, list.get(), &control) },
            Err(Error::Limit)
        );
        control.cancel();
        assert_eq!(
            unsafe { hardware_from_list(identifier, list.get(), &control) },
            Err(Error::Cancelled)
        );
    }
}
