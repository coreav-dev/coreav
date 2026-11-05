//! libvpx's VP8 and VP9 decoder API, declared from its public headers
//! (`vpx/vpx_codec.h`, `vpx/vpx_decoder.h`, `vpx/vpx_frame_buffer.h`, `vpx/vpx_image.h`,
//! `vpx/vp8dx.h`, decoder ABI version 12), behind a safe API: [`Vpx`] owns
//! one decoder instance and lends each decoded image as a [`View`] whose
//! rows are slices checked against the image's own geometry. Every
//! `unsafe` of the crate is here (`docs/unsafe-sites.md`).

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Mutex;

use coreav_decode::{BufferLease, BufferPlane, Error, ErrorKind, PictureAllocator, SharedFrame};

/// `VPX_DECODER_ABI_VERSION`: 3 + `VPX_CODEC_ABI_VERSION` (4 +
/// `VPX_IMAGE_ABI_VERSION`, 5). libvpx refuses to initialise a decoder for
/// any other version (`VPX_CODEC_ABI_MISMATCH`), so the layouts below are
/// never used with a library that declares others.
const DECODER_ABI_VERSION: c_int = 12;
/// `VPX_CODEC_CAP_HIGHBITDEPTH`.
const CAP_HIGH_BIT_DEPTH: c_long = 0x4;
/// `VP9D_SET_ROW_MT` (`VP8_DECODER_CTRL_ID_START` 256, then the 13th
/// entry after it).
const SET_ROW_MT: c_int = 268;
/// `VPX_IMG_FMT_HIGHBITDEPTH`: 16-bit samples.
pub(crate) const FMT_HIGH_BIT_DEPTH: c_int = 0x800;

/// `vpx_codec_ctx_t`.
#[repr(C)]
#[allow(dead_code)]
struct Context {
    name: *const c_char,
    iface: *const c_void,
    err: c_int,
    err_detail: *const c_char,
    init_flags: c_long,
    config: *const c_void,
    private: *mut c_void,
}

/// `vpx_codec_dec_cfg_t`.
#[repr(C)]
#[allow(dead_code)]
struct Config {
    threads: c_uint,
    w: c_uint,
    h: c_uint,
}

#[repr(C)]
struct FrameBuffer {
    data: *mut u8,
    size: usize,
    private: *mut c_void,
}

struct Allocation {
    buffer: BufferLease,
    base: usize,
    len: usize,
    slot: Option<u32>,
}

struct AllocatorState {
    allocator: PictureAllocator,
    error: Mutex<Option<Error>>,
}

fn callback_result(result: std::thread::Result<c_int>) -> c_int {
    match result {
        Ok(code) => code,
        Err(payload) => {
            std::mem::forget(payload);
            -1
        }
    }
}

unsafe extern "C" fn get_frame_buffer(
    private: *mut c_void,
    min_size: usize,
    fb: *mut FrameBuffer,
) -> c_int {
    callback_result(catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: registration supplies the boxed state until decoder destruction;
        // libvpx supplies a nonnull, exclusively accessed frame-buffer descriptor.
        let (state, fb) = unsafe { (&*private.cast::<AllocatorState>(), &mut *fb) };
        let result = (|| {
            let mut buffer = state.allocator.allocate(min_size)?;
            let slot = buffer.shared_slot();
            let bytes = buffer.bytes_mut();
            if bytes.len() < min_size {
                return Err(Error::new(
                    ErrorKind::Backend,
                    "allocator returned a short frame buffer",
                ));
            }
            bytes.fill(0);
            let data = bytes.as_mut_ptr();
            let len = bytes.len();
            let allocation = Box::new(Allocation {
                buffer: buffer.into(),
                base: data as usize,
                len,
                slot,
            });
            fb.data = data;
            fb.size = len;
            fb.private = Box::into_raw(allocation).cast();
            Ok(())
        })();
        match result {
            Ok(()) => 0,
            Err(error) => {
                let mut saved = state.error.lock().unwrap_or_else(|e| e.into_inner());
                if saved.is_none() {
                    *saved = Some(error);
                }
                -1
            }
        }
    })))
}

unsafe extern "C" fn release_frame_buffer(_: *mut c_void, fb: *mut FrameBuffer) -> c_int {
    callback_result(catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libvpx supplies a nonnull, exclusively accessed descriptor and
        // calls release only when it no longer references this allocation.
        let fb = unsafe { &mut *fb };
        let allocation = fb.private.cast::<Allocation>();
        fb.private = ptr::null_mut();
        fb.data = ptr::null_mut();
        fb.size = 0;
        if !allocation.is_null() {
            // SAFETY: get transferred this box to this descriptor; clearing it
            // above ensures that the owner is recovered at most once.
            drop(unsafe { Box::from_raw(allocation) });
        }
        0
    })))
}

/// `vpx_image_t`.
#[repr(C)]
#[allow(dead_code)]
struct Image {
    fmt: c_int,
    cs: c_int,
    range: c_int,
    w: c_uint,
    h: c_uint,
    bit_depth: c_uint,
    d_w: c_uint,
    d_h: c_uint,
    r_w: c_uint,
    r_h: c_uint,
    x_chroma_shift: c_uint,
    y_chroma_shift: c_uint,
    planes: [*mut u8; 4],
    stride: [c_int; 4],
    bps: c_int,
    user_priv: *mut c_void,
    img_data: *mut u8,
    img_data_owner: c_int,
    self_allocd: c_int,
    fb_priv: *mut c_void,
}

// The sizes the headers give on LP64 targets (checked with the C compiler
// against libvpx 1.14's headers).
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<Context>() == 56);
    assert!(size_of::<Config>() == 12);
    assert!(size_of::<FrameBuffer>() == 24);
    assert!(size_of::<Image>() == 136);
};

unsafe extern "C" {
    fn vpx_codec_vp8_dx() -> *const c_void;
    fn vpx_codec_vp9_dx() -> *const c_void;
    fn vpx_codec_get_caps(iface: *const c_void) -> c_long;
    fn vpx_codec_version_str() -> *const c_char;
    fn vpx_codec_err_to_string(err: c_int) -> *const c_char;
    fn vpx_codec_error_detail(ctx: *const Context) -> *const c_char;
    fn vpx_codec_dec_init_ver(
        ctx: *mut Context,
        iface: *const c_void,
        cfg: *const Config,
        flags: c_long,
        ver: c_int,
    ) -> c_int;
    fn vpx_codec_control_(ctx: *mut Context, ctrl_id: c_int, ...) -> c_int;
    fn vpx_codec_decode(
        ctx: *mut Context,
        data: *const u8,
        data_sz: c_uint,
        user_priv: *mut c_void,
        deadline: c_long,
    ) -> c_int;
    fn vpx_codec_get_frame(ctx: *mut Context, iter: *mut *const c_void) -> *const Image;
    fn vpx_codec_destroy(ctx: *mut Context) -> c_int;
    fn vpx_codec_set_frame_buffer_functions(
        ctx: *mut Context,
        get: unsafe extern "C" fn(*mut c_void, usize, *mut FrameBuffer) -> c_int,
        release: unsafe extern "C" fn(*mut c_void, *mut FrameBuffer) -> c_int,
        private: *mut c_void,
    ) -> c_int;
}

/// A libvpx error: its `vpx_codec_err_t` and libvpx's words for it.
#[derive(Debug)]
pub(crate) struct Failure {
    /// `vpx_codec_err_t`: 2 memory, 3 ABI mismatch, 4 incapable, 5 and 6
    /// unsupported, 7 corrupt, 8 invalid parameter.
    pub code: c_int,
    pub message: String,
    pub allocator_error: Option<Error>,
}

/// A C string libvpx returned, or `None` for a null pointer.
fn text(s: *const c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    // SAFETY: libvpx returns null or a NUL-terminated string that it keeps
    // (a static string, or the context's detail, read here before the
    // context is used again).
    let s = unsafe { CStr::from_ptr(s) };
    Some(s.to_string_lossy().chars().take(300).collect())
}

/// libvpx's version, `v1.14.0` for example.
pub(crate) fn version() -> String {
    // SAFETY: no arguments; returns a static string.
    text(unsafe { vpx_codec_version_str() }).unwrap_or_default()
}

/// Whether this libvpx decodes VP9 at more than 8 bits (profiles 2 and 3).
pub(crate) fn high_bit_depth() -> bool {
    // SAFETY: the interface pointer is libvpx's own static VP9 decoder
    // interface, which `vpx_codec_get_caps` only reads.
    unsafe { vpx_codec_get_caps(vpx_codec_vp9_dx()) & CAP_HIGH_BIT_DEPTH != 0 }
}

/// The libvpx decoder interfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interface {
    /// `vpx_codec_vp8_dx`.
    Vp8,
    /// `vpx_codec_vp9_dx`.
    Vp9,
}

/// One VP8 or VP9 decoder instance.
///
/// Boxed so that neither moves: libvpx keeps a pointer to the
/// configuration in the context (`config`) and reads it when it starts
/// decoding.
pub(crate) struct Vpx {
    context: Box<Context>,
    _config: Box<Config>,
    allocator: Option<Box<AllocatorState>>,
}

// SAFETY: a libvpx decoder instance may be used from any thread, one call
// at a time; `Vpx` gives no shared access (every call takes `&mut self`).
// Callback threads share only a Send + Sync allocator and a mutex; picture
// owners are Send + Sync and are released after libvpx's last reference.
unsafe impl Send for Vpx {}

impl Vpx {
    /// A decoder of `interface` on `threads` threads (at least 1); for
    /// VP9, with row-based multithreading when there are several.
    pub fn open(
        interface: Interface,
        threads: u32,
        allocator: Option<PictureAllocator>,
    ) -> Result<Self, Failure> {
        let mut context = Box::new(Context {
            name: ptr::null(),
            iface: ptr::null(),
            err: 0,
            err_detail: ptr::null(),
            init_flags: 0,
            config: ptr::null(),
            private: ptr::null_mut(),
        });
        let config = Box::new(Config {
            threads: threads.max(1),
            w: 0,
            h: 0,
        });
        // SAFETY: `context` and `config` are valid, boxed (they outlive the
        // decoder, which `Drop` destroys first) and laid out as the headers
        // of ABI version 12 declare; libvpx checks that version. The
        // interface is libvpx's own static VP8 or VP9 decoder interface.
        let err = unsafe {
            let iface = match interface {
                Interface::Vp8 => vpx_codec_vp8_dx(),
                Interface::Vp9 => vpx_codec_vp9_dx(),
            };
            vpx_codec_dec_init_ver(&mut *context, iface, &*config, 0, DECODER_ABI_VERSION)
        };
        if err != 0 {
            // SAFETY: a static string for any code.
            let message = text(unsafe { vpx_codec_err_to_string(err) }).unwrap_or_default();
            return Err(Failure {
                code: err,
                message,
                allocator_error: None,
            });
        }
        let mut vpx = Self {
            context,
            _config: config,
            allocator: if interface == Interface::Vp9 {
                allocator.map(|allocator| {
                    Box::new(AllocatorState {
                        allocator,
                        error: Mutex::new(None),
                    })
                })
            } else {
                None
            },
        };
        if let Some(state) = vpx.allocator.as_ref() {
            // SAFETY: an initialized VP9 decoder before its first decode; both
            // callbacks match the public headers. The boxed state stays at this
            // address until after vpx_codec_destroy releases its allocations.
            let err = unsafe {
                vpx_codec_set_frame_buffer_functions(
                    &mut *vpx.context,
                    get_frame_buffer,
                    release_frame_buffer,
                    (&**state as *const AllocatorState).cast_mut().cast(),
                )
            };
            if err != 0 {
                return Err(vpx.failure(err));
            }
        }
        if interface == Interface::Vp9 && threads > 1 {
            // SAFETY: an initialised VP9 decoder; `VP9D_SET_ROW_MT` takes one
            // `int`.
            let on: c_int = 1;
            let err = unsafe { vpx_codec_control_(&mut *vpx.context, SET_ROW_MT, on) };
            if err != 0 {
                return Err(vpx.failure(err));
            }
        }
        Ok(vpx)
    }

    fn failure(&self, code: c_int) -> Failure {
        // SAFETY: an initialised decoder; both return null or a string
        // valid until the decoder is used again, read at once.
        let detail = text(unsafe { vpx_codec_error_detail(&*self.context) });
        let name = text(unsafe { vpx_codec_err_to_string(code) }).unwrap_or_default();
        let message = match detail {
            Some(detail) => format!("{name}: {detail}"),
            None => name,
        };
        let allocator_error = self
            .allocator
            .as_ref()
            .and_then(|state| state.error.lock().unwrap_or_else(|e| e.into_inner()).take());
        Failure {
            code,
            message,
            allocator_error,
        }
    }

    /// Decodes one packet (one frame, or a VP9 superframe) and calls `each`
    /// with every image it makes ready, in order, until `each` fails.
    pub fn decode<E>(
        &mut self,
        data: &[u8],
        mut each: impl FnMut(View<'_>) -> Result<(), E>,
    ) -> Result<Result<(), E>, Failure> {
        let Ok(size) = c_uint::try_from(data.len()) else {
            return Err(Failure {
                code: 8,
                message: "packet larger than libvpx takes".into(),
                allocator_error: None,
            });
        };
        // SAFETY: an initialised decoder; `data` is valid for `size` bytes
        // for the call, and libvpx keeps no pointer into it after the call
        // (no input fragments were asked for). No user data, no deadline.
        let err = unsafe {
            vpx_codec_decode(&mut *self.context, data.as_ptr(), size, ptr::null_mut(), 0)
        };
        if err != 0 {
            return Err(self.failure(err));
        }
        let mut iter: *const c_void = ptr::null();
        loop {
            // SAFETY: an initialised decoder and an iterator that started
            // null, as the API asks.
            let image = unsafe { vpx_codec_get_frame(&mut *self.context, &mut iter) };
            // SAFETY: libvpx returns null or a valid image that stays valid
            // until the next `vpx_codec_decode` or `vpx_codec_destroy`; the
            // reference lives only for this call, which holds `&mut self`.
            let Some(image) = (unsafe { image.as_ref() }) else {
                return Ok(Ok(()));
            };
            if let Err(e) = each(View {
                image,
                external: self.allocator.is_some(),
            }) {
                return Ok(Err(e));
            }
        }
    }
}

impl Drop for Vpx {
    fn drop(&mut self) {
        // SAFETY: an initialised decoder, destroyed once; the boxes are
        // freed after this.
        unsafe {
            vpx_codec_destroy(&mut *self.context);
        }
    }
}

/// A decoded image, for the duration of [`Vpx::decode`]'s callback.
pub(crate) struct View<'a> {
    image: &'a Image,
    external: bool,
}

impl<'a> View<'a> {
    fn allocation(&self) -> Option<&Allocation> {
        if !self.external {
            return None;
        }
        // SAFETY: only the external-buffer context creates this view; the public
        // image contract returns get's private pointer in fb_priv. The allocation
        // is retained while this image is valid, through the decode callback.
        unsafe { self.image.fb_priv.cast::<Allocation>().as_ref() }
    }

    pub fn shared_backing(&self) -> Option<BufferLease> {
        let allocation = self.allocation()?;
        allocation.slot?;
        Some(allocation.buffer.clone())
    }

    pub fn shared_frame(&self) -> Option<SharedFrame> {
        if !self.external
            || (cfg!(target_endian = "big") && self.sample_bytes() != 1)
            || self.image.d_w > self.image.w
            || self.image.d_h > self.image.h
        {
            return None;
        }
        let allocation = self.allocation()?;
        let slot = allocation.slot?;
        let mut planes = Vec::with_capacity(3);
        for plane in 0..3 {
            let (samples, rows) = self.plane_size(plane)?;
            let row_bytes = samples.checked_mul(self.sample_bytes())?;
            let stride = usize::try_from(*self.image.stride.get(plane)?).ok()?;
            let base = *self.image.planes.get(plane)?;
            if base.is_null() || row_bytes == 0 || row_bytes > stride {
                return None;
            }
            let offset = (base as usize).checked_sub(allocation.base)?;
            let len = rows
                .checked_sub(1)?
                .checked_mul(stride)?
                .checked_add(row_bytes)?;
            if offset.checked_add(len)? > allocation.len {
                return None;
            }
            planes.push(BufferPlane {
                offset: u64::try_from(offset).ok()?,
                stride: u64::try_from(stride).ok()?,
                len: u64::try_from(len).ok()?,
            });
        }
        Some(SharedFrame { slot, planes })
    }

    /// `fmt`: `VPX_IMG_FMT_I420` (0x102), `I422` (0x105), `I444` (0x106),
    /// `I440` (0x107), with [`FMT_HIGH_BIT_DEPTH`] for 16-bit samples.
    pub fn format(&self) -> c_int {
        self.image.fmt
    }

    /// Bits per sample.
    pub fn bit_depth(&self) -> u32 {
        self.image.bit_depth
    }

    /// The displayed size (`d_w`, `d_h`): the frame's size.
    pub fn size(&self) -> (u32, u32) {
        (self.image.d_w, self.image.d_h)
    }

    /// The chroma planes' subsampling shifts.
    pub fn shifts(&self) -> (u32, u32) {
        (self.image.x_chroma_shift, self.image.y_chroma_shift)
    }

    /// Samples per row and rows of plane 0 (Y), 1 (U) or 2 (V), from the
    /// displayed size and the shifts, rounding up.
    pub fn plane_size(&self, plane: usize) -> Option<(usize, usize)> {
        let (w, h) = self.size();
        let (x, y) = match plane {
            0 => (0, 0),
            1 | 2 => self.shifts(),
            _ => return None,
        };
        if x > 1 || y > 1 {
            return None;
        }
        let samples = usize::try_from(w.checked_add((1 << x) - 1)? >> x).ok()?;
        let rows = usize::try_from(h.checked_add((1 << y) - 1)? >> y).ok()?;
        Some((samples, rows))
    }

    /// Bytes of one sample: 2 with [`FMT_HIGH_BIT_DEPTH`], else 1.
    pub fn sample_bytes(&self) -> usize {
        if self.image.fmt & FMT_HIGH_BIT_DEPTH != 0 {
            2
        } else {
            1
        }
    }

    /// Row `row` of `plane`: [`Self::plane_size`]'s samples, in
    /// [`Self::sample_bytes`] each (16-bit samples in the host's byte
    /// order). `None` past the plane's rows, or when the image's pointer or
    /// stride cannot hold the row.
    pub fn row(&self, plane: usize, row: usize) -> Option<&'a [u8]> {
        let (samples, rows) = self.plane_size(plane)?;
        if row >= rows || self.image.d_w > self.image.w || self.image.d_h > self.image.h {
            return None;
        }
        let base = *self.image.planes.get(plane)?;
        let stride = usize::try_from(*self.image.stride.get(plane)?).ok()?;
        let len = samples.checked_mul(self.sample_bytes())?;
        if base.is_null() || len > stride {
            return None;
        }
        let offset = row.checked_mul(stride)?;
        isize::try_from(offset.checked_add(len)?).ok()?;
        // SAFETY: libvpx's image contract (`vpx_image.h`): `planes[plane]`
        // points to the top-left sample of the plane, whose rows are
        // `stride[plane]` bytes apart, for the stored size `w` x `h`
        // (here at least the displayed size, checked above) subsampled by
        // the shifts, in samples of 16 bits under `VPX_IMG_FMT_HIGHBITDEPTH`.
        // So row `row` < the plane's rows starts `row * stride` bytes after
        // `base` and holds `len` <= `stride` bytes, the plane's displayed
        // samples. The image, and so the bytes, stay valid and unwritten for
        // `'a` (`Vpx::decode`'s callback).
        Some(unsafe { std::slice::from_raw_parts(base.add(offset), len) })
    }
}

#[cfg(test)]
mod allocator_tests {
    use super::*;
    use coreav_decode::{BufferAllocator, DecodeBuffer};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Dirty {
        drops: Arc<AtomicUsize>,
        data: Vec<u8>,
    }
    impl DecodeBuffer for Dirty {
        fn bytes_mut(&mut self) -> &mut [u8] {
            &mut self.data
        }
        fn shared_slot(&self) -> Option<u32> {
            Some(7)
        }
    }
    impl Drop for Dirty {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Provider {
        drops: Arc<AtomicUsize>,
        short: bool,
    }
    impl BufferAllocator for Provider {
        fn allocate(&self, len: usize) -> Result<Box<dyn DecodeBuffer>, Error> {
            Ok(Box::new(Dirty {
                drops: self.drops.clone(),
                data: vec![99; len - usize::from(self.short)],
            }))
        }
    }

    #[test]
    fn callbacks_zero_storage_and_keep_publication_alive_after_release() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut state = AllocatorState {
            allocator: Arc::new(Provider {
                drops: drops.clone(),
                short: false,
            }),
            error: Mutex::new(None),
        };
        let mut fb = FrameBuffer {
            data: ptr::null_mut(),
            size: 0,
            private: ptr::null_mut(),
        };
        assert_eq!(
            unsafe { get_frame_buffer((&mut state as *mut AllocatorState).cast(), 4096, &mut fb) },
            0
        );
        let allocation = unsafe { &*fb.private.cast::<Allocation>() };
        let publication = allocation.buffer.clone();
        assert!(
            unsafe { std::slice::from_raw_parts(fb.data, fb.size) }
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(unsafe { release_frame_buffer(ptr::null_mut(), &mut fb) }, 0);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        drop(publication);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_short_allocator_fails_without_publishing_a_pointer() {
        let mut state = AllocatorState {
            allocator: Arc::new(Provider {
                drops: Arc::new(AtomicUsize::new(0)),
                short: true,
            }),
            error: Mutex::new(None),
        };
        let mut fb = FrameBuffer {
            data: ptr::null_mut(),
            size: 0,
            private: ptr::null_mut(),
        };
        assert!(
            unsafe { get_frame_buffer((&mut state as *mut AllocatorState).cast(), 4096, &mut fb) }
                < 0
        );
        assert!(fb.private.is_null());
        assert!(fb.data.is_null());
    }
}
