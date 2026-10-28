//! NAL units: headers, the two framings streams arrive in, and the removal
//! of emulation prevention bytes (clauses 7.3.1 and 7.4.1, Annex B).
//!
//! Everything here is sans-IO: the byte-stream
//! splitter takes chunks of bytes and hands out the NAL units it has
//! completed, and the length-prefixed splitter is an iterator over one
//! sample.
#![forbid(unsafe_code)]

use crate::error::{Error, LimitKind, Result};
use crate::limits::Limits;

/// What a NAL unit carries (Table 7-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NalKind {
    /// A coded slice of a non-IDR picture.
    Slice,
    /// A coded slice data partition (A, B or C).
    Partition,
    /// A coded slice of an IDR picture.
    IdrSlice,
    /// Supplemental enhancement information.
    Sei,
    /// A sequence parameter set.
    Sps,
    /// A picture parameter set.
    Pps,
    /// An access unit delimiter.
    AccessUnitDelimiter,
    /// End of sequence.
    EndOfSequence,
    /// End of stream.
    EndOfStream,
    /// Filler data.
    Filler,
    /// A type this decoder passes over: parameter set extensions, the
    /// prefix and subset sequence parameter sets of Annexes G and H,
    /// auxiliary and redundant slices, and reserved or unspecified types.
    Other(u8),
}

/// The NAL unit header (clause 7.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalHeader {
    /// `nal_ref_idc`: zero where the unit is not used for reference.
    pub nal_ref_idc: u8,
    /// `nal_unit_type`.
    pub kind: NalKind,
}

impl NalHeader {
    /// Reads the one-byte header of a NAL unit.
    pub fn parse(nal: &[u8]) -> Result<NalHeader> {
        let &first = nal.first().ok_or(Error::Truncated("the NAL unit header"))?;
        if first & 0x80 != 0 {
            return Err(Error::Invalid("forbidden_zero_bit"));
        }
        let kind = match first & 0x1f {
            1 => NalKind::Slice,
            2..=4 => NalKind::Partition,
            5 => NalKind::IdrSlice,
            6 => NalKind::Sei,
            7 => NalKind::Sps,
            8 => NalKind::Pps,
            9 => NalKind::AccessUnitDelimiter,
            10 => NalKind::EndOfSequence,
            11 => NalKind::EndOfStream,
            12 => NalKind::Filler,
            other => NalKind::Other(other),
        };
        Ok(NalHeader {
            nal_ref_idc: (first >> 5) & 3,
            kind,
        })
    }

    /// Whether the unit holds a coded slice of a primary picture.
    pub fn is_slice(&self) -> bool {
        matches!(self.kind, NalKind::Slice | NalKind::IdrSlice)
    }
}

/// Removes every emulation_prevention_three_byte from a NAL unit's payload
/// (the bytes after the header), leaving the RBSP in `out`.
pub fn rbsp(payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    if out.capacity() < payload.len() {
        // A buffer too small is let go before the larger one is taken,
        // rather than grown, which would hold both for a moment: the
        // slice data the decoder holds stays at one NAL unit.
        *out = Vec::new();
    }
    out.reserve(payload.len());
    // A byte is an emulation_prevention_three_byte where it is 3 and the two
    // bytes before it are 0 (clause 7.4.1): a removed byte is a 3, never one
    // of the zeros before the next, so the rule needs no state. The bytes
    // between are copied in runs. Only a zero at `i` lets such a byte be at
    // `i + 1` or `i + 2`, so after any other byte the search moves on three.
    let mut from = 0;
    let mut i = 2;
    while let Some(&b) = payload.get(i) {
        if b == 0 {
            i += 1;
            continue;
        }
        if b == 3 && payload.get(i - 2..i) == Some(&[0, 0][..]) {
            out.extend_from_slice(payload.get(from..i).unwrap_or_default());
            from = i + 1;
        }
        i += 3;
    }
    out.extend_from_slice(payload.get(from..).unwrap_or_default());
}

/// The index of the next `0x000001` start code at or after `from`.
fn start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    loop {
        let w = data.get(i..i + 3)?;
        match w {
            [0, 0, 1] => return Some(i),
            // A start code cannot end in a byte above 1, so the next one
            // starts at least three bytes on.
            [_, _, c] if *c > 1 => i += 3,
            _ => i += 1,
        }
    }
}

/// Drops the trailing zero bytes of a NAL unit: they are the `zero_byte` of
/// the next start code, or `trailing_zero_8bits` (clause B.1.1).
fn trim_trailing_zeros(data: &[u8]) -> &[u8] {
    let end = data.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    data.get(..end).unwrap_or(data)
}

/// The NAL units of a complete Annex B byte stream, without their start
/// codes.
pub fn annex_b(data: &[u8]) -> AnnexBUnits<'_> {
    AnnexBUnits {
        data,
        next: start_code(data, 0).map(|i| i + 3),
    }
}

/// The iterator [`annex_b`] returns.
#[derive(Clone, Debug)]
pub struct AnnexBUnits<'a> {
    data: &'a [u8],
    next: Option<usize>,
}

impl<'a> Iterator for AnnexBUnits<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            let start = self.next?;
            let rest = self.data.get(start..)?;
            let (unit, next) = match start_code(self.data, start) {
                Some(end) => (self.data.get(start..end)?, Some(end + 3)),
                None => (rest, None),
            };
            self.next = next;
            let unit = trim_trailing_zeros(unit);
            if !unit.is_empty() {
                return Some(unit);
            }
            self.next?;
        }
    }
}

/// An Annex B byte stream arriving in chunks.
///
/// The splitter holds at most one NAL unit plus the chunk being scanned, and
/// fails with [`LimitKind::NalBytes`] rather than growing past the limit.
#[derive(Debug)]
pub struct AnnexB {
    buf: Vec<u8>,
    /// Index in `buf` of the first byte of the NAL unit being collected.
    start: Option<usize>,
    /// Index in `buf` where the search for the next start code continues.
    scan: usize,
    max_nal_bytes: usize,
}

impl AnnexB {
    /// A splitter that holds at most `limits.max_nal_bytes` of one unit.
    pub fn new(limits: &Limits) -> AnnexB {
        AnnexB {
            buf: Vec::new(),
            start: None,
            scan: 0,
            max_nal_bytes: limits.max_nal_bytes,
        }
    }

    /// Adds `chunk` and calls `unit` with every NAL unit it completes.
    ///
    /// The last unit of a stream is completed by [`AnnexB::finish`], since
    /// only the end of the stream ends it.
    ///
    /// While this runs, the splitter holds the unit it is collecting and
    /// `chunk`; between calls it holds only the unit. No unit larger than
    /// `max_nal_bytes` is handed out, whether it arrived whole in one
    /// chunk or grew across several: the call fails with
    /// [`LimitKind::NalBytes`] instead, the unit is dropped, and a later
    /// call picks the stream up at the next start code.
    pub fn push(&mut self, chunk: &[u8], unit: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        self.compact();
        self.buf.extend_from_slice(chunk);
        self.split(unit, false)?;
        if self.open_bytes() > self.max_nal_bytes {
            // Drop the unit that broke the limit and pick the stream up
            // at the next start code, which is what the check inside
            // `split` does for a unit that arrives whole. Two bytes stay,
            // in case a start code straddles this chunk and the next.
            let keep = self.buf.len().saturating_sub(2);
            self.buf.drain(..keep);
            self.start = None;
            self.scan = 0;
            return Err(Error::Limit(LimitKind::NalBytes));
        }
        Ok(())
    }

    /// Bytes of the NAL unit being collected, if one is open.
    fn open_bytes(&self) -> usize {
        self.start
            .map_or(0, |start| self.buf.len().saturating_sub(start))
    }

    /// Ends the stream: calls `unit` with the NAL unit the last chunk left
    /// open, if any.
    pub fn finish(&mut self, unit: impl FnMut(&[u8]) -> Result<()>) -> Result<()> {
        self.split(unit, true)
    }

    /// Drops the bytes before the unit being collected.
    fn compact(&mut self) {
        let keep = match self.start {
            Some(s) => s.min(self.scan),
            None => self.scan.min(self.buf.len()),
        };
        if keep == 0 {
            return;
        }
        self.buf.drain(..keep);
        self.scan -= keep;
        if let Some(s) = self.start.as_mut() {
            *s -= keep;
        }
    }

    fn split(&mut self, mut unit: impl FnMut(&[u8]) -> Result<()>, end: bool) -> Result<()> {
        // Disjoint borrows: `unit` sees `buf` while the indices change.
        let AnnexB {
            buf,
            start,
            scan,
            max_nal_bytes,
        } = self;
        let max_nal_bytes = *max_nal_bytes;
        loop {
            let Some(s) = *start else {
                match start_code(buf, *scan) {
                    Some(i) => {
                        *start = Some(i + 3);
                        *scan = i + 3;
                    }
                    None if end => {
                        // No start code at all: nothing of this stream is a
                        // NAL unit, and nothing is worth keeping.
                        buf.clear();
                        *scan = 0;
                        return Ok(());
                    }
                    None => {
                        *scan = buf.len().saturating_sub(2);
                        return Ok(());
                    }
                }
                continue;
            };
            match start_code(buf, (*scan).max(s)) {
                Some(next) => {
                    let found = buf.get(s..next).unwrap_or_default();
                    let found = trim_trailing_zeros(found);
                    *start = None;
                    *scan = next;
                    if found.len() > max_nal_bytes {
                        return Err(Error::Limit(LimitKind::NalBytes));
                    }
                    if !found.is_empty() {
                        unit(found)?;
                    }
                }
                None if end => {
                    let found = trim_trailing_zeros(buf.get(s..).unwrap_or_default());
                    let too_big = found.len() > max_nal_bytes;
                    if !found.is_empty() && !too_big {
                        unit(found)?;
                    }
                    buf.clear();
                    *start = None;
                    *scan = 0;
                    return if too_big {
                        Err(Error::Limit(LimitKind::NalBytes))
                    } else {
                        Ok(())
                    };
                }
                None => {
                    // Two bytes may still be the head of a start code.
                    *scan = buf.len().saturating_sub(2).max(s);
                    return Ok(());
                }
            }
        }
    }
}

/// The NAL units of one length-prefixed sample, as MP4 and Matroska carry
/// them (ISO/IEC 14496-15 `avcC`, `lengthSizeMinusOne + 1` bytes per
/// prefix).
///
/// A prefix that does not fit, or that is larger than the rest of the
/// sample, ends the iteration with [`Error::Truncated`].
pub fn length_prefixed(sample: &[u8], length_size: usize) -> LengthPrefixedUnits<'_> {
    LengthPrefixedUnits {
        rest: sample,
        length_size,
        done: false,
    }
}

/// The iterator [`length_prefixed`] returns.
#[derive(Clone, Debug)]
pub struct LengthPrefixedUnits<'a> {
    rest: &'a [u8],
    length_size: usize,
    done: bool,
}

impl<'a> Iterator for LengthPrefixedUnits<'a> {
    type Item = Result<&'a [u8]>;

    fn next(&mut self) -> Option<Result<&'a [u8]>> {
        // A loop, not recursion: a sample of zero-length units must not grow
        // the stack.
        loop {
            if self.done || self.rest.is_empty() {
                return None;
            }
            let mut fail = |what| {
                self.done = true;
                Some(Err(Error::Truncated(what)))
            };
            if !(1..=4).contains(&self.length_size) {
                return fail("a NAL unit length prefix of 1 to 4 bytes");
            }
            let Some(prefix) = self.rest.get(..self.length_size) else {
                return fail("a NAL unit length prefix");
            };
            let len = prefix
                .iter()
                .fold(0usize, |v, &b| (v << 8) | usize::from(b));
            let body = self.rest.get(self.length_size..);
            match body.and_then(|b| b.split_at_checked(len)) {
                Some((unit, rest)) => {
                    self.rest = rest;
                    if !unit.is_empty() {
                        return Some(Ok(unit));
                    }
                }
                None => return fail("a NAL unit"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(stream: &[u8], chunk: usize) -> Vec<Vec<u8>> {
        let mut splitter = AnnexB::new(&Limits::default());
        let mut out = Vec::new();
        for part in stream.chunks(chunk) {
            splitter
                .push(part, |nal| {
                    out.push(nal.to_vec());
                    Ok(())
                })
                .expect("push");
        }
        splitter
            .finish(|nal| {
                out.push(nal.to_vec());
                Ok(())
            })
            .expect("finish");
        out
    }

    const STREAM: &[u8] = &[
        0, 0, 0, 1, 0x67, 1, 2, // an SPS, four-byte start code
        0, 0, 1, 0x68, 3, // a PPS
        0, 0, 0, 0, 1, 0x65, 0, 0, 3, 4, 0, 0, // an IDR slice, trailing zeros
    ];

    #[test]
    fn the_two_splitters_agree_whatever_the_chunking() {
        let want: Vec<Vec<u8>> = annex_b(STREAM).map(<[u8]>::to_vec).collect();
        assert_eq!(
            want,
            vec![vec![0x67, 1, 2], vec![0x68, 3], vec![0x65, 0, 0, 3, 4],]
        );
        for chunk in 1..=STREAM.len() + 1 {
            assert_eq!(collect(STREAM, chunk), want, "chunks of {chunk}");
        }
    }

    #[test]
    fn a_whole_unit_larger_than_the_limit_is_not_handed_out() {
        // The unit arrives complete inside one chunk, so nothing grows
        // past the limit while it is collected; it is still too large to
        // hand to a decoder (found by the decode_limited fuzz target).
        let limits = Limits {
            max_nal_bytes: 8,
            ..Limits::default()
        };
        let mut splitter = AnnexB::new(&limits);
        let mut stream = vec![0, 0, 1, 0x65];
        stream.extend(std::iter::repeat_n(b'x', 18));
        stream.extend_from_slice(&[0, 0, 1]);
        let mut seen = 0;
        let result = splitter.push(&stream, |_| {
            seen += 1;
            Ok(())
        });
        assert_eq!(result, Err(Error::Limit(LimitKind::NalBytes)));
        assert_eq!(seen, 0);
    }

    #[test]
    fn a_nal_unit_larger_than_the_limit_fails() {
        let limits = Limits {
            max_nal_bytes: 8,
            ..Limits::default()
        };
        let mut splitter = AnnexB::new(&limits);
        let mut stream = vec![0, 0, 1, 0x65];
        stream.extend(std::iter::repeat_n(0x42, 32));
        let e = splitter.push(&stream, |_| Ok(())).unwrap_err();
        assert_eq!(e, Error::Limit(LimitKind::NalBytes));
        // The stream goes on at the next start code.
        let mut seen = 0;
        splitter
            .push(&[0, 0, 1, 0x61, 9], |nal| {
                seen += 1;
                assert_eq!(nal, &[0x65]);
                Ok(())
            })
            .expect("push");
        assert_eq!(seen, 0, "the oversized unit is not handed out");
        splitter
            .finish(|nal| {
                assert_eq!(nal, &[0x61, 9]);
                Ok(())
            })
            .expect("finish");
        // The limit counts one unit, not the chunk: a chunk far larger than
        // the limit, holding only small units, is fine.
        let mut splitter = AnnexB::new(&limits);
        let mut stream = Vec::new();
        for _ in 0..100 {
            stream.extend_from_slice(&[0, 0, 1, 0x61, 7]);
        }
        let mut units = 0;
        splitter
            .push(&stream, |_| {
                units += 1;
                Ok(())
            })
            .expect("push");
        splitter
            .finish(|_| {
                units += 1;
                Ok(())
            })
            .expect("finish");
        assert_eq!(units, 100);
    }

    #[test]
    fn a_sample_of_empty_units_ends_without_recursing() {
        // 64 KiB of zero-length NAL units: a loop, never 65,536 stack
        // frames.
        let sample = vec![0u8; 64 << 10];
        assert_eq!(length_prefixed(&sample, 1).count(), 0);
        assert_eq!(length_prefixed(&sample, 4).count(), 0);
    }

    #[test]
    fn a_stream_without_a_start_code_leaves_nothing_behind() {
        let mut splitter = AnnexB::new(&Limits::default());
        splitter.push(&[1, 2, 3, 4, 5], |_| Ok(())).expect("push");
        splitter.finish(|_| Ok(())).expect("finish");
        assert!(splitter.buf.is_empty());
    }

    #[test]
    fn the_splitter_holds_only_the_unit_it_collects() {
        let mut splitter = AnnexB::new(&Limits::default());
        let mut units = 0;
        for _ in 0..64 {
            splitter
                .push(&[0, 0, 1, 0x61, 7, 7, 7, 7], |_| {
                    units += 1;
                    Ok(())
                })
                .expect("push");
            assert!(
                splitter.buf.len() <= 16,
                "held {} bytes",
                splitter.buf.len()
            );
        }
        assert_eq!(units, 63);
    }

    #[test]
    fn emulation_prevention_bytes_are_removed() {
        let mut out = Vec::new();
        rbsp(&[0, 0, 3, 1, 0, 0, 3, 0, 0, 3], &mut out);
        assert_eq!(out, vec![0, 0, 1, 0, 0, 0, 0]);
        // A three byte that does not follow two zeros stays.
        rbsp(&[3, 0, 3, 1], &mut out);
        assert_eq!(out, vec![3, 0, 3, 1]);
    }

    /// The byte-at-a-time removal of clause 7.3.1, to check [`rbsp`]'s runs
    /// against.
    fn rbsp_by_bytes(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut zeros = 0;
        for &b in payload {
            if zeros >= 2 && b == 3 {
                zeros = 0;
                continue;
            }
            zeros = if b == 0 { zeros + 1 } else { 0 };
            out.push(b);
        }
        out
    }

    #[test]
    fn runs_remove_what_the_byte_loop_removes() {
        // Bytes drawn mostly from 0 and 3, so that runs of zeros, three
        // bytes after two, three and more zeros, and three bytes in a row
        // all occur, at every length up to 40.
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut out = Vec::new();
        for n in 0..4000usize {
            let len = n % 41;
            let payload: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    match x % 8 {
                        0..=3 => 0,
                        4 | 5 => 3,
                        6 => 1,
                        _ => (x >> 8) as u8,
                    }
                })
                .collect();
            rbsp(&payload, &mut out);
            assert_eq!(out, rbsp_by_bytes(&payload), "{payload:?}");
        }
        for case in [
            &[0, 0, 0, 3][..],
            &[0, 0, 3, 3],
            &[0, 0, 3, 0, 0, 3],
            &[0, 0, 0, 0, 3, 0],
        ] {
            rbsp(case, &mut out);
            assert_eq!(out, rbsp_by_bytes(case), "{case:?}");
        }
    }

    #[test]
    fn headers_name_their_unit() {
        let sps = NalHeader::parse(&[0x67]).expect("header");
        assert_eq!(sps.kind, NalKind::Sps);
        assert_eq!(sps.nal_ref_idc, 3);
        assert!(!sps.is_slice());
        assert!(NalHeader::parse(&[0x65]).expect("header").is_slice());
        assert_eq!(
            NalHeader::parse(&[0x14]).expect("header").kind,
            NalKind::Other(20)
        );
        assert_eq!(
            NalHeader::parse(&[]),
            Err(Error::Truncated("the NAL unit header"))
        );
        assert_eq!(
            NalHeader::parse(&[0x80]),
            Err(Error::Invalid("forbidden_zero_bit"))
        );
    }

    #[test]
    fn length_prefixed_units_stop_at_a_bad_prefix() {
        let sample = [0, 0, 0, 2, 0x65, 9, 0, 0, 0, 1, 0x68];
        let units: Vec<&[u8]> = length_prefixed(&sample, 4)
            .map(|u| u.expect("unit"))
            .collect();
        assert_eq!(units, vec![&[0x65u8, 9][..], &[0x68][..]]);
        // A length past the end of the sample.
        let short = [0, 0, 0, 9, 0x65];
        let mut units = length_prefixed(&short, 4);
        assert!(matches!(units.next(), Some(Err(Error::Truncated(_)))));
        assert!(units.next().is_none());
    }
}
