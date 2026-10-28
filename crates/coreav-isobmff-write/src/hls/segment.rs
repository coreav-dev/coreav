//! Media segments: one movie fragment (`moof` and `mdat`, ISO/IEC 14496-12
//! 8.8) per segment, holding one track.

use crate::ExtractError;
use crate::boxes::{Body, Node, header_len, write_header};
use crate::copy::COPY_BUFFER;
use crate::moov::overflow;
use coreav_isobmff::{Error, Source};
use std::io::Write;

/// The output track's `track_ID` in every init and media segment.
pub(crate) const TRACK_ID: u32 = 1;

/// `sample_depends_on = 2` (does not depend on other samples), 14496-12
/// 8.8.3.1: a sync sample.
const SYNC_FLAGS: u32 = 0x0200_0000;
/// `sample_is_non_sync_sample = 1`; dependencies not stated.
const NON_SYNC_FLAGS: u32 = 0x0001_0000;

/// One sample of a segment, as the index stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sample {
    /// Absolute offset of the sample data in the source.
    pub(crate) offset: u64,
    pub(crate) size: u32,
    pub(crate) duration: u32,
    /// Composition offset as stored in the source: signed, or up to
    /// `u32::MAX` from a version 0 table.
    pub(crate) cto: i64,
    pub(crate) sync: bool,
}

impl Sample {
    fn flags(&self) -> u32 {
        if self.sync {
            SYNC_FLAGS
        } else {
            NON_SYNC_FLAGS
        }
    }
}

/// Which `trun` fields are stored per sample and which defaults `tfhd`
/// carries (14496-12 8.8.7, 8.8.8).
struct Layout {
    count: u32,
    /// Every sample has this duration.
    default_duration: Option<u32>,
    /// Every sample after the first has these flags.
    default_flags: Option<u32>,
    /// The first sample's flags, when they differ from the others'.
    first_flags: Option<u32>,
    /// Composition offsets are stored; version 1 (signed) when any is
    /// negative.
    cto: bool,
    signed: bool,
}

impl Layout {
    /// The layout of `samples` and their total bytes, in one pass: the
    /// table is walked once per segment, since every sample of every
    /// segment goes through it while the index is built.
    fn of(samples: &[Sample]) -> Result<(Self, u64), Error> {
        let count = u32::try_from(samples.len()).map_err(|_| overflow())?;
        let (mut lowest, mut highest) = (0i64, 0i64);
        let mut same_duration = true;
        // Whether every sample after the first has the second's flags.
        let mut same_flags = true;
        let mut any_cto = false;
        let mut data_bytes = 0u64;
        let first = samples.first();
        let rest = samples.get(1..).unwrap_or_default();
        let second_flags = rest.first().map(Sample::flags);
        for sample in samples {
            lowest = lowest.min(sample.cto);
            highest = highest.max(sample.cto);
            any_cto |= sample.cto != 0;
            data_bytes = data_bytes
                .checked_add(u64::from(sample.size))
                .ok_or_else(overflow)?;
        }
        for sample in rest {
            same_duration &= first.is_none_or(|f| f.duration == sample.duration);
            same_flags &= second_flags == Some(sample.flags());
        }
        // Version 1 when any offset is negative, else version 0, whose
        // offsets are unsigned; one `trun` cannot hold both kinds.
        let signed = lowest < 0;
        let range = if signed {
            i64::from(i32::MIN)..=i64::from(i32::MAX)
        } else {
            0..=i64::from(u32::MAX)
        };
        if !range.contains(&lowest) || !range.contains(&highest) {
            return Err(Error::Unsupported {
                reason: "signed and unsigned composition offsets in one segment",
            });
        }
        let default_duration = first.map(|f| f.duration).filter(|_| same_duration);
        let (default_flags, first_flags) = match (first, second_flags) {
            (Some(f), None) => (Some(f.flags()), None),
            (Some(f), Some(second)) if same_flags => {
                let first_flags = (f.flags() != second).then_some(f.flags());
                (Some(second), first_flags)
            }
            _ => (None, None),
        };
        Ok((
            Self {
                count,
                default_duration,
                default_flags,
                first_flags,
                cto: any_cto,
                signed,
            },
            data_bytes,
        ))
    }

    fn tfhd(&self) -> Node<'static> {
        // default-base-is-moof: data offsets count from the `moof` box.
        let mut flags = 0x02_0000;
        let mut payload = TRACK_ID.to_be_bytes().to_vec();
        if let Some(duration) = self.default_duration {
            flags |= 0x08;
            payload.extend_from_slice(&duration.to_be_bytes());
        }
        if let Some(sample_flags) = self.default_flags {
            flags |= 0x20;
            payload.extend_from_slice(&sample_flags.to_be_bytes());
        }
        Node::full(b"tfhd", 0, flags, &payload)
    }

    /// The `trun` box. `samples` is `None` when only the box's size is
    /// wanted: the per-sample table is then left out of the tree as its
    /// own length, which the node writes as zeros and no caller writes.
    fn trun(&self, samples: Option<&[Sample]>, data_offset: i32) -> Node<'static> {
        let per_duration = self.default_duration.is_none();
        let per_flags = self.default_flags.is_none();
        // data-offset-present, sample-size-present.
        let mut flags = 0x01 | 0x200;
        if self.first_flags.is_some() {
            flags |= 0x04;
        }
        if per_duration {
            flags |= 0x100;
        }
        if per_flags {
            flags |= 0x400;
        }
        if self.cto {
            flags |= 0x800;
        }
        let mut head = Vec::with_capacity(12);
        head.extend_from_slice(&self.count.to_be_bytes());
        head.extend_from_slice(&data_offset.to_be_bytes());
        if let Some(first) = self.first_flags {
            head.extend_from_slice(&first.to_be_bytes());
        }
        let version = u8::from(self.signed);
        let Some(samples) = samples else {
            let per_sample = 4
                + u64::from(per_duration) * 4
                + u64::from(per_flags) * 4
                + u64::from(self.cto) * 4;
            let table = per_sample.saturating_mul(u64::from(self.count));
            return Node::full_split(b"trun", version, flags, &head, Body::Zeros(table));
        };
        let mut payload = head;
        payload.reserve(samples.len().saturating_mul(16));
        for sample in samples {
            if per_duration {
                payload.extend_from_slice(&sample.duration.to_be_bytes());
            }
            payload.extend_from_slice(&sample.size.to_be_bytes());
            if per_flags {
                payload.extend_from_slice(&sample.flags().to_be_bytes());
            }
            if self.cto {
                // In range for the version (checked in `of`): the low 32
                // bits, read as signed in version 1 and unsigned in 0.
                let bits = (sample.cto & 0xffff_ffff) as u32;
                payload.extend_from_slice(&bits.to_be_bytes());
            }
        }
        Node::full(b"trun", version, flags, &payload)
    }
}

/// One segment's fragment, planned before it is written: the `moof` box's
/// length, the `mdat` header's and the samples' bytes. The fields of
/// `mfhd` and `tfdt` have fixed widths, so the lengths do not depend on
/// the sequence number or the decode time; the per-sample table is left
/// out and counted.
pub(crate) struct Plan {
    layout: Layout,
    moof_len: u64,
    mdat_header: u64,
    /// The samples' bytes, the `mdat` payload.
    pub(crate) data_bytes: u64,
}

impl Plan {
    pub(crate) fn of(samples: &[Sample]) -> Result<Self, Error> {
        let (layout, data_bytes) = Layout::of(samples)?;
        // The data offset field has a fixed width too, so the size does
        // not depend on its value.
        let moof_len = moof(0, 0, &layout, None, 0).len();
        Ok(Self {
            layout,
            moof_len,
            mdat_header: header_len(data_bytes),
            data_bytes,
        })
    }

    /// The whole segment's bytes: the header and the samples.
    pub(crate) fn len(&self) -> Result<u64, Error> {
        self.moof_len
            .checked_add(self.mdat_header)
            .and_then(|head| head.checked_add(self.data_bytes))
            .ok_or_else(overflow)
    }

    /// The `moof` box of segment `sequence` (counted from 1) followed by
    /// the `mdat` header, for `samples` decoded from `base_decode_time`.
    /// `samples` must be the ones the plan was made from.
    fn write_header(
        &self,
        sequence: u32,
        base_decode_time: u64,
        samples: &[Sample],
    ) -> Result<Vec<u8>, Error> {
        let data_offset = self
            .moof_len
            .checked_add(self.mdat_header)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(Error::Unsupported {
                reason: "a segment's sample table does not fit a movie fragment",
            })?;
        let mut out = Vec::with_capacity(usize::try_from(self.moof_len).map_err(|_| overflow())?);
        moof(
            sequence,
            base_decode_time,
            &self.layout,
            Some(samples),
            data_offset,
        )
        .write(&mut out)?;
        write_header(&mut out, b"mdat", self.data_bytes)?;
        Ok(out)
    }
}

fn moof(
    sequence: u32,
    base_decode_time: u64,
    layout: &Layout,
    samples: Option<&[Sample]>,
    data_offset: i32,
) -> Node<'static> {
    let mfhd = Node::full(b"mfhd", 0, 0, &sequence.to_be_bytes());
    let tfdt = Node::full(b"tfdt", 1, 0, &base_decode_time.to_be_bytes());
    let traf = Node::parent(
        b"traf",
        vec![layout.tfhd(), tfdt, layout.trun(samples, data_offset)],
    );
    Node::parent(b"moof", vec![mfhd, traf])
}

/// The `moof` box of segment `sequence` (counted from 1) followed by the
/// `mdat` header, for `samples` decoded from `base_decode_time`.
pub(crate) fn header(
    sequence: u32,
    base_decode_time: u64,
    samples: &[Sample],
) -> Result<Vec<u8>, Error> {
    Plan::of(samples)?.write_header(sequence, base_decode_time, samples)
}

/// A source byte range copied into a segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyRange {
    /// Absolute offset of the first byte.
    pub offset: u64,
    /// Length in bytes, at least 1.
    pub len: u64,
}

impl CopyRange {
    /// Offset just past the range.
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }
}

/// One media segment: [`header`](Self::header) followed by the source bytes
/// of [`ranges`](Self::ranges), in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaSegment {
    header: Vec<u8>,
    ranges: Vec<CopyRange>,
    source_len: u64,
    len: u64,
    samples: u32,
}

impl MediaSegment {
    pub(crate) fn new(
        sequence: u32,
        base_decode_time: u64,
        samples: &[Sample],
        source_len: u64,
    ) -> Result<Self, Error> {
        let header = header(sequence, base_decode_time, samples)?;
        // Samples that follow each other in the source are copied as one
        // range.
        let mut ranges: Vec<CopyRange> = Vec::new();
        for sample in samples.iter().filter(|s| s.size > 0) {
            match ranges.last_mut() {
                Some(last) if last.end() == sample.offset => {
                    last.len = last
                        .len
                        .checked_add(u64::from(sample.size))
                        .ok_or_else(overflow)?;
                }
                _ => ranges.push(CopyRange {
                    offset: sample.offset,
                    len: u64::from(sample.size),
                }),
            }
        }
        let data = ranges
            .iter()
            .try_fold(0u64, |sum, r| sum.checked_add(r.len))
            .ok_or_else(overflow)?;
        let len = u64::try_from(header.len())
            .ok()
            .and_then(|h| h.checked_add(data))
            .ok_or_else(overflow)?;
        Ok(Self {
            header,
            ranges,
            source_len,
            len,
            samples: u32::try_from(samples.len()).map_err(|_| overflow())?,
        })
    }

    /// The `moof` box and the `mdat` header.
    pub fn header(&self) -> &[u8] {
        &self.header
    }

    /// The source byte ranges that follow the header, in order. Adjacent
    /// samples are merged into one range.
    pub fn ranges(&self) -> &[CopyRange] {
        &self.ranges
    }

    /// Bytes of the whole segment.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the segment is empty (never: a segment has a header).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Samples in the segment.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Length of the source the index was built from; [`write`](Self::write)
    /// refuses a source of another length.
    pub fn source_len(&self) -> u64 {
        self.source_len
    }

    /// Writes the segment to `out`, copying the ranges from `source`
    /// through one buffer of at most 64 KiB. Returns the bytes written.
    ///
    /// Fails with [`ExtractError::SourceChanged`] before writing anything
    /// when `source` does not have the length the index was built from.
    pub fn write<S: Source, W: Write>(
        &self,
        mut source: S,
        mut out: W,
    ) -> Result<u64, ExtractError> {
        if source.len()? != self.source_len {
            return Err(ExtractError::SourceChanged);
        }
        out.write_all(&self.header)?;
        let largest = self.ranges.iter().map(|r| r.len).max().unwrap_or(0);
        let capacity = usize::try_from(largest).map_or(COPY_BUFFER, |l| l.min(COPY_BUFFER));
        let mut buffer = vec![0; capacity];
        for range in &self.ranges {
            let mut at = range.offset;
            let mut left = range.len;
            while left > 0 {
                let step = usize::try_from(left).map_or(capacity, |l| l.min(capacity));
                let piece = buffer.get_mut(..step).ok_or_else(overflow)?;
                source.read_exact_at(at, piece)?;
                out.write_all(piece)?;
                let step = u64::try_from(step).map_err(|_| overflow())?;
                at = at.checked_add(step).ok_or_else(overflow)?;
                left = left.saturating_sub(step);
            }
        }
        out.flush()?;
        Ok(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(offset: u64, size: u32, cto: i64, sync: bool) -> Sample {
        Sample {
            offset,
            size,
            duration: 512,
            cto,
            sync,
        }
    }

    #[test]
    fn header_points_at_the_data() {
        let samples = [
            sample(100, 10, 1024, true),
            sample(110, 5, -512, false),
            sample(200, 7, 0, false),
        ];
        let segment = MediaSegment::new(3, 9_000, &samples, 1_000).unwrap();
        let h = segment.header();
        let moof_len = u32::from_be_bytes(h[..4].try_into().unwrap()) as usize;
        assert_eq!(&h[4..8], b"moof");
        assert_eq!(&h[moof_len + 4..moof_len + 8], b"mdat");
        assert_eq!(h.len(), moof_len + 8);
        assert_eq!(
            u32::from_be_bytes(h[moof_len..moof_len + 4].try_into().unwrap()),
            8 + 22
        );
        // trun: version 1 for the negative offset; data offset after the
        // mdat header.
        let trun = h.windows(4).position(|w| w == b"trun").unwrap();
        assert_eq!(h[trun + 4], 1);
        let flags = u32::from_be_bytes(h[trun + 4..trun + 8].try_into().unwrap()) & 0xff_ffff;
        assert_eq!(flags, 0x01 | 0x04 | 0x200 | 0x800);
        let offset = i32::from_be_bytes(h[trun + 12..trun + 16].try_into().unwrap());
        assert_eq!(offset as usize, h.len());
        assert_eq!(
            segment.ranges(),
            &[
                CopyRange {
                    offset: 100,
                    len: 15
                },
                CopyRange {
                    offset: 200,
                    len: 7
                }
            ]
        );
        assert_eq!(segment.len(), h.len() as u64 + 22);
        let source: Vec<u8> = (0..1_000u32).map(|i| i as u8).collect();
        let mut out = Vec::new();
        segment
            .write(coreav_isobmff::SliceSource::new(&source), &mut out)
            .unwrap();
        assert_eq!(out.len() as u64, segment.len());
        assert_eq!(&out[h.len()..h.len() + 15], &source[100..115]);
        assert_eq!(&out[h.len() + 15..], &source[200..207]);
        let shorter = &source[..999];
        assert_eq!(
            segment.write(coreav_isobmff::SliceSource::new(shorter), Vec::new()),
            Err(ExtractError::SourceChanged)
        );
    }

    #[test]
    fn audio_uses_defaults() {
        let samples = [sample(0, 4, 0, true), sample(4, 6, 0, true)];
        let h = header(1, 0, &samples).unwrap();
        let tfhd = h.windows(4).position(|w| w == b"tfhd").unwrap();
        let flags = u32::from_be_bytes(h[tfhd + 4..tfhd + 8].try_into().unwrap());
        assert_eq!(flags, 0x02_0000 | 0x08 | 0x20);
        let trun = h.windows(4).position(|w| w == b"trun").unwrap();
        let flags = u32::from_be_bytes(h[trun + 4..trun + 8].try_into().unwrap());
        assert_eq!(flags, 0x01 | 0x200);
    }
}
