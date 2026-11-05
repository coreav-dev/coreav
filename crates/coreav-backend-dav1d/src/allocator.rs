//! dav1d's picture-allocation contract, with storage supplied by the host.

use std::sync::{Arc, Mutex};

use coreav_decode::{
    BufferAllocator, BufferLease, BufferPlane, DecodeBuffer, Error, ErrorKind, PictureAllocator,
    SharedFrame,
};
use dav1d::{PictureAllocation, PictureParameters, PixelLayout};

#[derive(Clone)]
pub(crate) struct Allocator(pub PictureAllocator);

pub(crate) struct Allocation {
    pub lease: BufferLease,
    slot: Option<u32>,
    offsets: [usize; 3],
    strides: [usize; 2],
}

impl Allocation {
    pub fn shared_frame(
        &self,
        width: u32,
        height: u32,
        format: coreav_frame::PixelFormat,
    ) -> Option<SharedFrame> {
        if cfg!(target_endian = "big") && format.sample_bytes() == 2 {
            return None;
        }
        let mut planes = Vec::new();
        for index in 0..format.plane_count() {
            let (_, rows) = format.plane_size(index, width, height)?;
            let row = format.row_bytes(index, width, height)?;
            let stride = *self.strides.get(usize::from(index != 0))?;
            let len = stride
                .checked_mul(usize::try_from(rows).ok()?.checked_sub(1)?)?
                .checked_add(row)?;
            planes.push(BufferPlane {
                offset: u64::try_from(*self.offsets.get(index)?).ok()?,
                stride: u64::try_from(stride).ok()?,
                len: u64::try_from(len).ok()?,
            });
        }
        Some(SharedFrame {
            slot: self.slot?,
            planes,
        })
    }
}

fn geometry(p: &PictureParameters) -> Option<([usize; 3], [usize; 2], usize)> {
    let width = usize::try_from(p.width()).ok()?.checked_add(127)? & !127;
    let height = usize::try_from(p.height()).ok()?.checked_add(127)? & !127;
    let sample = match p.bit_depth() {
        8 => 1,
        10 | 12 => 2,
        _ => return None,
    };
    if width == 0 || height == 0 {
        return None;
    }
    let (x, y, chroma) = match p.pixel_layout() {
        PixelLayout::I400 => (0, 0, false),
        PixelLayout::I420 => (1, 1, true),
        PixelLayout::I422 => (1, 0, true),
        PixelLayout::I444 => (0, 0, true),
    };
    let luma = width.checked_mul(sample)?.checked_add(63)? & !63;
    let chroma_stride = (width >> x).checked_mul(sample)?.checked_add(63)? & !63;
    let y_len = luma.checked_mul(height)?.checked_add(64)?;
    let c_len = if chroma {
        chroma_stride.checked_mul(height >> y)?.checked_add(64)?
    } else {
        0
    };
    let v = y_len.checked_add(c_len)?;
    let len = v.checked_add(c_len)?.checked_add(63)?;
    isize::try_from(len).ok()?;
    Some(([0, y_len, v], [luma, chroma_stride], len))
}

unsafe impl dav1d::PictureAllocator for Allocator {
    type AllocatorData = Allocation;

    unsafe fn alloc_picture(
        &self,
        p: &PictureParameters,
    ) -> Result<PictureAllocation<Allocation>, dav1d::Error> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let short = || dav1d::Error::NotEnoughMemory;
            let (offsets, strides, len) = geometry(p).ok_or_else(short)?;
            let mut storage = self.0.allocate(len).map_err(|_| short())?;
            let slot = storage.shared_slot();
            let bytes = storage.bytes_mut();
            if bytes.len() < len {
                return Err(short());
            }
            let base = bytes.as_mut_ptr();
            let alignment = (64 - (base as usize & 63)) & 63;
            let offsets = offsets.map(|offset| offset + alignment);
            let mut data = [std::ptr::null_mut(); 3];
            for (index, (dst, offset)) in data.iter_mut().zip(offsets).enumerate() {
                if index == 0 || p.pixel_layout() != PixelLayout::I400 {
                    *dst = unsafe { base.add(offset) };
                }
            }
            Ok(PictureAllocation {
                data,
                stride: strides.map(|stride| stride as isize),
                allocator_data: Allocation {
                    lease: storage.into(),
                    slot,
                    offsets,
                    strides,
                },
            })
        }))
        .unwrap_or_else(|payload| {
            std::mem::forget(payload);
            Err(dav1d::Error::NotEnoughMemory)
        })
    }

    unsafe fn release_picture(&self, allocation: PictureAllocation<Allocation>) {
        if let Err(payload) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(allocation)))
        {
            std::mem::forget(payload);
        }
    }
}

#[derive(Clone)]
struct Heap {
    state: Arc<Mutex<HeapState>>,
    budget: usize,
}

#[derive(Default)]
struct HeapState {
    free: Vec<Vec<u8>>,
    bytes: usize,
}

struct HeapBuffer {
    data: Vec<u8>,
    pool: Heap,
}

impl DecodeBuffer for HeapBuffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl Drop for HeapBuffer {
    fn drop(&mut self) {
        if let Ok(mut state) = self.pool.state.lock() {
            if state.free.len() < 32 {
                state.free.push(std::mem::take(&mut self.data));
            } else {
                state.bytes = state.bytes.saturating_sub(self.data.capacity());
            }
        }
    }
}

impl BufferAllocator for Heap {
    fn allocate(&self, len: usize) -> Result<Box<dyn DecodeBuffer>, Error> {
        let fail = || Error::new(ErrorKind::Limit, "out of memory for a picture");
        let mut state = self.state.lock().map_err(|_| fail())?;
        let mut data = match state.free.iter().position(|data| data.capacity() >= len) {
            Some(index) => state.free.swap_remove(index),
            None => {
                while state.bytes.checked_add(len).is_none_or(|n| n > self.budget) {
                    let Some(data) = state.free.pop() else {
                        return Err(fail());
                    };
                    state.bytes = state.bytes.saturating_sub(data.capacity());
                }
                let mut data = Vec::new();
                data.try_reserve_exact(len).map_err(|_| fail())?;
                state.bytes = state.bytes.checked_add(data.capacity()).ok_or_else(fail)?;
                data
            }
        };
        data.resize(len, 0);
        drop(state);
        Ok(Box::new(HeapBuffer {
            data,
            pool: self.clone(),
        }))
    }
}

pub(crate) fn heap(budget: u64) -> PictureAllocator {
    Arc::new(Heap {
        state: Arc::new(Mutex::new(HeapState::default())),
        budget: usize::try_from(budget).unwrap_or(usize::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_and_cached_heap_capacity_share_one_budget() {
        let heap = heap(4096);
        let first = heap.allocate(4096).expect("first");
        let decoder = BufferLease::from(first);
        let publication = decoder.clone();
        assert!(heap.allocate(1).is_err());
        drop(decoder);
        assert!(heap.allocate(1).is_err());
        drop(publication);
        let small = heap.allocate(1).expect("reuse");
        assert!(heap.allocate(1).is_err());
        drop(small);
        assert!(heap.allocate(4096).is_ok());
        assert!(heap.allocate(usize::MAX).is_err());
    }
}
