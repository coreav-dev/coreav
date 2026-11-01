//! The parent's checks of a frame descriptor from the untrusted worker,
//! and the copy of its planes (`docs/design/decode.md`, "Descriptors").
//! Nothing here trusts the worker: every field is checked, with checked
//! arithmetic, before a byte of the frame is read, and a failed check
//! creates no view. A frame lent in its slot instead of copied
//! (`parent.rs`) is read only inside the ranges [`check`] returns.

#![forbid(unsafe_code)]

use std::ops::Range;

use coreav_decode::{Error, ErrorKind, Limits};
use coreav_frame::{Frame, Picture, PixelFormat, Pixels, Plane};

use crate::protocol::{FrameDescriptor, INLINE, format_from_code};

/// Memory the worker may write while the parent reads it: read only by
/// copying a range out, never through a reference (`shm`).
pub trait SlotMemory {
    /// Its length in bytes.
    fn len(&self) -> usize;

    /// Whether it is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies `out.len()` bytes from offset `start` into `out`; `None` when
    /// the range is not inside the memory.
    fn copy_out(&self, start: usize, out: &mut [u8]) -> Option<()>;
}

/// Plain memory, for tests and fuzzing.
impl SlotMemory for Vec<u8> {
    fn len(&self) -> usize {
        self.as_slice().len()
    }

    fn copy_out(&self, start: usize, out: &mut [u8]) -> Option<()> {
        let end = start.checked_add(out.len())?;
        out.copy_from_slice(self.get(start..end)?);
        Some(())
    }
}

/// The shared memory a descriptor may point into.
#[derive(Clone, Copy)]
pub struct Slots<'a> {
    /// The whole mapping.
    pub memory: &'a dyn SlotMemory,
    /// Number of slots.
    pub count: u32,
    /// Bytes per slot.
    pub size: u64,
}

/// A frame the parent accepted.
#[derive(Debug)]
pub struct Accepted {
    /// The frame, in memory the parent owns.
    pub frame: Frame,
    /// The slot to acknowledge, for a frame in shared memory.
    pub slot: Option<u32>,
    /// Bytes of the frame's planes.
    pub bytes: u64,
}

fn violation(what: &str) -> Error {
    Error::new(
        ErrorKind::Backend,
        format!("worker sent an invalid frame descriptor: {what}"),
    )
}

/// A descriptor that passed [`check`].
#[derive(Debug)]
pub struct Checked {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// The slot, for a frame in shared memory.
    pub slot: Option<u32>,
    /// Bytes of the frame's planes.
    pub bytes: u64,
    /// Each plane's bytes, in the whole mapping for a frame in a slot, else
    /// in the inline data, and its stride.
    pub planes: Vec<(Range<usize>, usize)>,
}

/// Validates `descriptor` and copies its planes out of the slot it names
/// (or out of `inline`, the data that followed it), into a [`Frame`].
pub fn accept(
    descriptor: &FrameDescriptor,
    inline: &[u8],
    expected_seq: u64,
    slots: Option<Slots<'_>>,
    limits: &Limits,
) -> Result<Accepted, Error> {
    let checked = check(descriptor, inline.len(), expected_seq, slots, limits)?;
    let mut planes = Vec::with_capacity(checked.planes.len());
    for (range, stride) in checked.planes {
        let mut data = Vec::new();
        data.try_reserve_exact(range.len())
            .map_err(|_| Error::new(ErrorKind::Limit, "out of memory for a frame"))?;
        data.resize(range.len(), 0);
        // The one read of the worker's bytes: copied once, then never
        // read from the slot again.
        let copied = match (checked.slot, slots) {
            (Some(_), Some(slots)) => slots.memory.copy_out(range.start, &mut data),
            _ => inline.get(range).map(|source| data.copy_from_slice(source)),
        };
        copied.ok_or_else(|| violation("plane outside the slot"))?;
        planes.push(Plane::new(data, stride));
    }
    let picture = Picture::new(checked.width, checked.height, checked.format, planes)
        .map_err(|e| violation(&e.to_string()))?;
    Ok(Accepted {
        frame: Frame {
            pts: descriptor.pts,
            color: descriptor.color,
            pixels: Pixels::Cpu(picture),
        },
        slot: checked.slot,
        bytes: checked.bytes,
    })
}

/// Validates `descriptor` (with `inline_len` bytes of data after it) and
/// returns where its planes' bytes are.
///
/// Checks, in order: the sequence number is `expected_seq`; the slot
/// exists (or the frame is inline and nothing else is); the pixel format
/// is one the frame model knows; width and height are within `limits`;
/// there is one plane per format plane; for each plane, the stride covers a
/// row, the length is exactly `stride × (rows − 1) + row bytes`, and
/// `offset + length` lies inside the slot.
pub fn check(
    descriptor: &FrameDescriptor,
    inline_len: usize,
    expected_seq: u64,
    slots: Option<Slots<'_>>,
    limits: &Limits,
) -> Result<Checked, Error> {
    if descriptor.seq != expected_seq {
        return Err(violation("sequence number out of order"));
    }
    // Where the frame's bytes are: inline, or a slot at `base` of the
    // shared memory, `region_len` bytes long.
    let (base, region_len, slot) = if descriptor.slot == INLINE {
        (0, inline_len, None)
    } else {
        if inline_len != 0 {
            return Err(violation("data after a shared-memory descriptor"));
        }
        let slots = slots.ok_or_else(|| violation("slot without shared memory"))?;
        if descriptor.slot >= slots.count {
            return Err(violation("no such slot"));
        }
        let start = u64::from(descriptor.slot)
            .checked_mul(slots.size)
            .ok_or_else(|| violation("slot offset overflows"))?;
        let end = start
            .checked_add(slots.size)
            .ok_or_else(|| violation("slot end overflows"))?;
        let range = to_range(start, end)?;
        if range.end > slots.memory.len() {
            return Err(violation("slot outside the mapping"));
        }
        (range.start, range.len(), Some(descriptor.slot))
    };
    let format =
        format_from_code(descriptor.format).ok_or_else(|| violation("unknown pixel format"))?;
    let (width, height) = (descriptor.width, descriptor.height);
    // The size check comes before any plane arithmetic.
    limits.check_frame(width, height, 0)?;
    if descriptor.planes.len() != format.plane_count() {
        return Err(violation("wrong number of planes"));
    }
    let ranges = plane_ranges(descriptor, format, region_len)?;
    let bytes = ranges
        .iter()
        .try_fold(0u64, |sum, (r, _)| {
            sum.checked_add(u64::try_from(r.len()).ok()?)
        })
        .ok_or_else(|| violation("frame size overflows"))?;
    limits.check_frame(width, height, bytes)?;
    let planes = ranges
        .into_iter()
        .map(|(range, stride)| {
            let start = range.start.checked_add(base);
            let end = range.end.checked_add(base);
            match (start, end) {
                (Some(start), Some(end)) => Ok((start..end, stride)),
                _ => Err(violation("plane outside the slot")),
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(Checked {
        width,
        height,
        format,
        slot,
        bytes,
        planes,
    })
}

/// Each plane's byte range in the slot and its stride, checked.
fn plane_ranges(
    descriptor: &FrameDescriptor,
    format: PixelFormat,
    region_len: usize,
) -> Result<Vec<(Range<usize>, usize)>, Error> {
    let region_len = u64::try_from(region_len).map_err(|_| violation("slot size"))?;
    let mut out = Vec::with_capacity(descriptor.planes.len());
    for (index, plane) in descriptor.planes.iter().enumerate() {
        let row = format
            .row_bytes(index, descriptor.width, descriptor.height)
            .and_then(|r| u64::try_from(r).ok())
            .ok_or_else(|| violation("row size overflows"))?;
        let (_, rows) = format
            .plane_size(index, descriptor.width, descriptor.height)
            .ok_or_else(|| violation("no such plane"))?;
        if plane.stride < row {
            return Err(violation("stride shorter than a row"));
        }
        let expected = u64::from(rows)
            .checked_sub(1)
            .and_then(|r| r.checked_mul(plane.stride))
            .and_then(|b| b.checked_add(row))
            .ok_or_else(|| violation("plane size overflows"))?;
        if plane.len != expected {
            return Err(violation("plane length does not match its geometry"));
        }
        let end = plane
            .offset
            .checked_add(plane.len)
            .ok_or_else(|| violation("plane end overflows"))?;
        if end > region_len {
            return Err(violation("plane outside the slot"));
        }
        let stride = usize::try_from(plane.stride).map_err(|_| violation("stride"))?;
        out.push((to_range(plane.offset, end)?, stride));
    }
    Ok(out)
}

fn to_range(start: u64, end: u64) -> Result<Range<usize>, Error> {
    let s = usize::try_from(start).map_err(|_| violation("offset does not fit"))?;
    let e = usize::try_from(end).map_err(|_| violation("offset does not fit"))?;
    Ok(s..e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PlaneDescriptor;
    use coreav_frame::ColorInfo;

    fn descriptor(slot: u32, planes: Vec<PlaneDescriptor>) -> FrameDescriptor {
        FrameDescriptor {
            seq: 0,
            slot,
            pts: Some(1),
            width: 4,
            height: 2,
            format: (0, 1, 8),
            color: ColorInfo::default(),
            planes,
        }
    }

    fn good_planes() -> Vec<PlaneDescriptor> {
        vec![
            PlaneDescriptor {
                offset: 0,
                stride: 4,
                len: 8,
            },
            PlaneDescriptor {
                offset: 8,
                stride: 2,
                len: 2,
            },
            PlaneDescriptor {
                offset: 10,
                stride: 2,
                len: 2,
            },
        ]
    }

    #[test]
    fn accepts_a_valid_frame() {
        let memory: Vec<u8> = (0..32).collect();
        let slots = Slots {
            memory: &memory,
            count: 2,
            size: 16,
        };
        let d = descriptor(1, good_planes());
        let a = accept(&d, &[], 0, Some(slots), &Limits::default()).expect("accepted");
        assert_eq!(a.slot, Some(1));
        assert_eq!(a.bytes, 12);
        let Pixels::Cpu(p) = &a.frame.pixels else {
            panic!("cpu")
        };
        assert_eq!(p.row(0, 0), Some(&[16u8, 17, 18, 19][..]));
        assert_eq!(p.row(2, 0), Some(&[26u8, 27][..]));
        let inline: Vec<u8> = (0..12).collect();
        let a = accept(
            &descriptor(INLINE, good_planes()),
            &inline,
            0,
            None,
            &Limits::default(),
        )
        .expect("inline");
        assert_eq!(a.slot, None);
    }

    #[test]
    fn rejects_hostile_descriptors() {
        let memory = vec![0u8; 32];
        let slots = Some(Slots {
            memory: &memory,
            count: 2,
            size: 16,
        });
        let limits = Limits::default();
        let bad = |d: FrameDescriptor, inline: &[u8], seq: u64| {
            accept(&d, inline, seq, slots, &limits).is_err()
        };
        assert!(bad(descriptor(0, good_planes()), &[], 1));
        assert!(bad(descriptor(2, good_planes()), &[], 0));
        assert!(bad(descriptor(0, good_planes()), &[1], 0));
        let mut far = good_planes();
        far[2].offset = 15;
        assert!(bad(descriptor(1, far), &[], 0));
        let mut wrap = good_planes();
        wrap[0].offset = u64::MAX - 3;
        assert!(bad(descriptor(0, wrap), &[], 0));
        let mut narrow = good_planes();
        narrow[0].stride = 3;
        narrow[0].len = 7;
        assert!(bad(descriptor(0, narrow), &[], 0));
        let mut long = good_planes();
        long[1].len = 3;
        assert!(bad(descriptor(0, long), &[], 0));
        let mut huge = descriptor(0, good_planes());
        huge.width = u32::MAX;
        assert!(bad(huge, &[], 0));
        let mut format = descriptor(0, good_planes());
        format.format = (0, 1, 7);
        assert!(bad(format, &[], 0));
        let mut fewer = good_planes();
        fewer.pop();
        assert!(bad(descriptor(0, fewer), &[], 0));
        assert!(bad(descriptor(INLINE, good_planes()), &[0; 11], 0));
        assert!(accept(&descriptor(0, good_planes()), &[], 0, None, &limits).is_err());
    }
}
