//! The Opus packet layer (RFC 6716 section 3 and Appendix B).

use crate::Error;

/// Most frames in a packet: 120 ms of 2.5 ms frames (RFC 6716 3.2.5).
pub const MAX_FRAMES: usize = 48;

/// Most samples per channel in a packet, at 48 kHz: 120 ms (\[R5\]).
pub const MAX_PACKET_SAMPLES: u32 = 5760;

/// Largest frame in bytes ([R2]), also the largest length the one- or
/// two-byte length coding of 3.2.1 can state.
const MAX_FRAME_BYTES: usize = 1275;

/// The layers a packet uses (RFC 6716 Table 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// SILK only.
    Silk,
    /// SILK and CELT.
    Hybrid,
    /// CELT only.
    Celt,
}

/// The audio bandwidth a packet codes (RFC 6716 Table 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bandwidth {
    /// Narrowband, 4 kHz.
    Narrow,
    /// Medium-band, 6 kHz.
    Medium,
    /// Wideband, 8 kHz.
    Wide,
    /// Super-wideband, 12 kHz.
    SuperWide,
    /// Fullband, 20 kHz.
    Full,
}

/// The table-of-contents byte (RFC 6716 3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toc(pub u8);

impl Toc {
    /// The configuration number, 0 to 31.
    pub fn config(self) -> u8 {
        self.0 >> 3
    }

    /// The packet's mode.
    pub fn mode(self) -> Mode {
        match self.config() {
            0..=11 => Mode::Silk,
            12..=15 => Mode::Hybrid,
            _ => Mode::Celt,
        }
    }

    /// The packet's audio bandwidth.
    pub fn bandwidth(self) -> Bandwidth {
        match self.config() {
            0..=3 | 16..=19 => Bandwidth::Narrow,
            4..=7 => Bandwidth::Medium,
            8..=11 | 20..=23 => Bandwidth::Wide,
            12 | 13 | 24..=27 => Bandwidth::SuperWide,
            _ => Bandwidth::Full,
        }
    }

    /// Samples per channel of each frame at 48 kHz: 10, 20, 40 or 60 ms
    /// for SILK, 10 or 20 ms for Hybrid, 2.5, 5, 10 or 20 ms for CELT.
    pub fn frame_samples(self) -> u32 {
        // Each size is twice the one before, from 10 ms for SILK and
        // Hybrid and 2.5 ms for CELT; SILK reaches 60 ms instead of 80.
        let config = u32::from(self.config());
        match self.mode() {
            Mode::Silk if config & 3 == 3 => 2880,
            Mode::Silk => 480 << (config & 3),
            Mode::Hybrid => 480 << (config & 1),
            Mode::Celt => 120 << (config & 3),
        }
    }

    /// Whether the frames are coded in stereo (`s`).
    pub fn stereo(self) -> bool {
        self.0 & 0x04 != 0
    }

    /// The frame count code `c`, 0 to 3.
    pub fn code(self) -> u8 {
        self.0 & 3
    }
}

/// One Opus packet, cut into its frames.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    toc: Toc,
    count: usize,
    frames: [&'a [u8]; MAX_FRAMES],
    padding: usize,
}

impl<'a> Packet<'a> {
    /// Reads a packet whose length the container gives (RFC 6716 3.2):
    /// every byte of `data` belongs to it.
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
        Self::read(data, false).map(|(packet, _)| packet)
    }

    /// Reads a self-delimited packet (RFC 6716 Appendix B) from the start
    /// of `data`; returns it and the bytes it takes.
    pub fn parse_self_delimited(data: &'a [u8]) -> Result<(Self, usize), Error> {
        Self::read(data, true)
    }

    /// The TOC byte.
    pub fn toc(&self) -> Toc {
        self.toc
    }

    /// The compressed frames, in order; a frame of length 0 is one the
    /// encoder did not send or that was lost (RFC 6716 3.2.1).
    pub fn frames(&self) -> &[&'a [u8]] {
        self.frames.get(..self.count).unwrap_or_default()
    }

    /// Samples per channel of the packet at 48 kHz.
    pub fn samples(&self) -> u32 {
        // At most 48 frames: no overflow.
        self.count as u32 * self.toc.frame_samples()
    }

    /// Bytes of Opus padding, those that state its size included.
    pub fn padding(&self) -> usize {
        self.padding
    }

    fn read(data: &'a [u8], delimited: bool) -> Result<(Self, usize), Error> {
        let mut reader = Reader { data, at: 0 };
        // [R1].
        let toc = Toc(reader
            .byte()
            .map_err(|_| Error::Invalid("empty Opus packet"))?);
        // The frames are either all of one size (codes 0 and 1, CBR code
        // 3) or of sizes stated before them but for the last (code 2, VBR
        // code 3). The size not stated, the implicit one, is what the
        // packet's end leaves, or in a self-delimited packet its own field.
        let mut count = 1;
        let mut equal = true;
        let mut stated = [0usize; MAX_FRAMES];
        // Opus padding: the bytes at the packet's end, and those with the
        // ones before the frames that state its size.
        let mut trailing = 0usize;
        let mut padding = 0usize;
        match toc.code() {
            0 => {}
            1 => count = 2,
            2 => {
                count = 2;
                equal = false;
                stated[0] = reader
                    .length()
                    .map_err(|_| Error::Invalid("code 2 Opus packet without its length"))?;
            }
            _ => {
                let byte = reader
                    .byte()
                    .map_err(|_| Error::Invalid("code 3 Opus packet without its frame count"))?;
                count = usize::from(byte & 0x3F);
                // [R5].
                if count == 0 || count as u32 * toc.frame_samples() > MAX_PACKET_SAMPLES {
                    return Err(Error::Invalid("Opus packet longer than 120 ms"));
                }
                if byte & 0x40 != 0 {
                    loop {
                        let size = reader
                            .byte()
                            .map_err(|_| Error::Invalid("Opus packet ends in its padding size"))?;
                        let added = if size == 255 { 254 } else { usize::from(size) };
                        trailing += added;
                        padding += 1 + added;
                        if size != 255 {
                            break;
                        }
                    }
                }
                if byte & 0x80 != 0 {
                    equal = false;
                    for length in stated.iter_mut().take(count - 1) {
                        *length = reader
                            .length()
                            .map_err(|_| Error::Invalid("Opus packet ends in its frame lengths"))?;
                    }
                }
            }
        }
        let header = reader.at;
        let implicit = if delimited {
            reader
                .length()
                .map_err(|_| Error::Invalid("self-delimited Opus packet without its length"))?
        } else {
            let explicit: usize = if equal {
                0
            } else {
                stated.iter().take(count - 1).sum()
            };
            let left = data
                .len()
                .checked_sub(header + trailing + explicit)
                .ok_or(Error::Invalid("Opus packet shorter than its header states"))?;
            if equal {
                // [R3], [R6].
                if left % count != 0 {
                    return Err(Error::Invalid(
                        "Opus packet's frames do not divide into equal sizes",
                    ));
                }
                left / count
            } else {
                left
            }
        };
        // [R2].
        if implicit > MAX_FRAME_BYTES {
            return Err(Error::Invalid("Opus frame longer than 1275 bytes"));
        }
        let empty: &'a [u8] = &[];
        let mut frames = [empty; MAX_FRAMES];
        let mut at = reader.at;
        for (i, frame) in frames.iter_mut().enumerate().take(count) {
            let length = match stated.get(i) {
                Some(&length) if !equal && i + 1 < count => length,
                _ => implicit,
            };
            let end = at + length;
            *frame = data
                .get(at..end)
                .ok_or(Error::Invalid("Opus packet shorter than its frames"))?;
            at = end;
        }
        let used = at + trailing;
        if used > data.len() {
            return Err(Error::Invalid("Opus packet shorter than its padding"));
        }
        let packet = Self {
            toc,
            count,
            frames,
            padding,
        };
        Ok((packet, used))
    }
}

/// Cuts the sample of a multistream track (RFC 7845 3, the Opus-in-ISOBMFF
/// mapping 4.3.3) into its `streams` packets: each self-delimited but the
/// last. Calls `each` with the stream's index and its packet, in order,
/// and returns the samples per channel, the same for every stream. Every
/// stream's packet is cut and checked before `each` sees the first, so a
/// damaged sample decodes in no stream rather than in some.
pub fn for_each_stream<'a>(
    data: &'a [u8],
    streams: usize,
    each: impl FnMut(usize, &Packet<'a>) -> Result<(), Error>,
) -> Result<u32, Error> {
    if streams > 1 {
        split(data, streams, |_, _| Ok(()))?;
    }
    split(data, streams, each)
}

fn split<'a>(
    data: &'a [u8],
    streams: usize,
    mut each: impl FnMut(usize, &Packet<'a>) -> Result<(), Error>,
) -> Result<u32, Error> {
    let mut rest = data;
    let mut samples = None;
    for stream in 0..streams {
        let packet = if stream + 1 == streams {
            Packet::parse(rest)?
        } else {
            let (packet, used) = Packet::parse_self_delimited(rest)?;
            rest = rest.get(used..).unwrap_or_default();
            packet
        };
        match samples {
            None => samples = Some(packet.samples()),
            Some(n) if n != packet.samples() => {
                return Err(Error::Invalid(
                    "streams of an Opus packet differ in duration",
                ));
            }
            Some(_) => {}
        }
        each(stream, &packet)?;
    }
    samples.ok_or(Error::Invalid("Opus track without streams"))
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, ()> {
        let byte = *self.data.get(self.at).ok_or(())?;
        self.at += 1;
        Ok(byte)
    }

    /// A frame length in the one- or two-byte coding of RFC 6716 3.2.1.
    fn length(&mut self) -> Result<usize, ()> {
        let first = self.byte()?;
        if first < 252 {
            return Ok(usize::from(first));
        }
        let second = self.byte()?;
        Ok(usize::from(second) * 4 + usize::from(first))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lengths(packet: &Packet<'_>) -> Vec<usize> {
        packet.frames().iter().map(|f| f.len()).collect()
    }

    #[test]
    fn toc_fields() {
        // RFC 6716 Figures 8 to 11.
        let silk = Toc(1 << 3);
        assert_eq!(
            (silk.mode(), silk.bandwidth()),
            (Mode::Silk, Bandwidth::Narrow)
        );
        assert_eq!(silk.frame_samples(), 960);
        let celt = Toc(29 << 3 | 1);
        assert_eq!(
            (celt.mode(), celt.bandwidth()),
            (Mode::Celt, Bandwidth::Full)
        );
        assert_eq!((celt.frame_samples(), celt.code()), (240, 1));
        let hybrid = Toc(15 << 3 | 3);
        assert_eq!(
            (hybrid.mode(), hybrid.bandwidth()),
            (Mode::Hybrid, Bandwidth::Full)
        );
        assert_eq!(hybrid.frame_samples(), 960);
        let stereo = Toc(31 << 3 | 4 | 3);
        assert!(stereo.stereo());
        assert_eq!(Toc(3 << 3).frame_samples(), 2880);
        assert_eq!(Toc(12 << 3).bandwidth(), Bandwidth::SuperWide);
        assert_eq!(Toc(16 << 3).frame_samples(), 120);
    }

    #[test]
    fn codes_0_to_2() {
        let p = Packet::parse(&[8, 1, 2, 3]).unwrap();
        assert_eq!(lengths(&p), [3]);
        assert_eq!(p.samples(), 960);
        let p = Packet::parse(&[8 | 1, 1, 2, 3, 4]).unwrap();
        assert_eq!(lengths(&p), [2, 2]);
        assert_eq!(p.frames()[1], [3, 4]);
        // [R3].
        assert!(Packet::parse(&[8 | 1, 1, 2, 3]).is_err());
        let p = Packet::parse(&[8 | 2, 1, 9, 7, 7]).unwrap();
        assert_eq!(lengths(&p), [1, 2]);
        assert_eq!(p.frames()[0], [9]);
        // [R4]: the only valid two-byte code 2 packet has two empty frames.
        assert_eq!(lengths(&Packet::parse(&[8 | 2, 0]).unwrap()), [0, 0]);
        assert!(Packet::parse(&[8 | 2, 1]).is_err());
        assert!(Packet::parse(&[8 | 2, 252]).is_err());
        assert!(Packet::parse(&[8 | 2]).is_err());
        // [R1].
        assert!(Packet::parse(&[]).is_err());
    }

    #[test]
    fn two_byte_lengths() {
        // 252 + 4 * 1 = 256 bytes for the first frame.
        let mut data = vec![8 | 2, 252, 1];
        data.extend(std::iter::repeat_n(5, 256 + 10));
        let p = Packet::parse(&data).unwrap();
        assert_eq!(lengths(&p), [256, 10]);
    }

    #[test]
    fn r2_limits_implicit_lengths() {
        let mut data = vec![8];
        data.extend(std::iter::repeat_n(0, 1275));
        assert!(Packet::parse(&data).is_ok());
        data.push(0);
        assert!(Packet::parse(&data).is_err());
    }

    #[test]
    fn code_3_cbr_and_padding() {
        // Figure 11: four 20 ms CELT frames of equal size.
        let p = Packet::parse(&[31 << 3 | 4 | 3, 4, 1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(lengths(&p), [2, 2, 2, 2]);
        assert_eq!(p.samples(), 3840);
        // Padding of 2 bytes, stated in one byte.
        let p = Packet::parse(&[31 << 3 | 3, 0x40 | 2, 2, 1, 2, 3, 4, 0, 0]).unwrap();
        assert_eq!(lengths(&p), [2, 2]);
        assert_eq!(p.padding(), 3);
        // 255 then 0: 254 bytes of padding.
        let mut data = vec![31 << 3 | 3, 0x40 | 1, 255, 0, 1, 2];
        data.extend(std::iter::repeat_n(0, 254));
        let p = Packet::parse(&data).unwrap();
        assert_eq!(lengths(&p), [2]);
        assert_eq!(p.padding(), 256);
        // [R6]: frames that do not divide equally, padding past the end.
        assert!(Packet::parse(&[31 << 3 | 3, 2, 1, 2, 3]).is_err());
        assert!(Packet::parse(&[31 << 3 | 3, 0x40 | 1, 9, 1]).is_err());
        assert!(Packet::parse(&[31 << 3 | 3, 0x40 | 1, 255]).is_err());
        assert!(Packet::parse(&[31 << 3 | 3]).is_err());
    }

    #[test]
    fn code_3_vbr() {
        // Figure 10: two 20 ms Hybrid frames of different sizes.
        let p = Packet::parse(&[15 << 3 | 3, 0x80 | 2, 1, 7, 8, 9]).unwrap();
        assert_eq!(lengths(&p), [1, 2]);
        assert_eq!(p.frames()[1], [8, 9]);
        // [R7].
        assert!(Packet::parse(&[15 << 3 | 3, 0x80 | 2, 5, 7]).is_err());
        assert!(Packet::parse(&[15 << 3 | 3, 0x80 | 3, 1]).is_err());
    }

    #[test]
    fn r5_limits_duration() {
        // 48 frames of 2.5 ms: 120 ms.
        let p = Packet::parse(&[16 << 3 | 3, 48]).unwrap();
        assert_eq!((p.frames().len(), p.samples()), (48, 5760));
        assert!(Packet::parse(&[16 << 3 | 3, 49]).is_err());
        // Three 60 ms SILK frames: 180 ms.
        assert!(Packet::parse(&[3 << 3 | 3, 3]).is_err());
        assert!(Packet::parse(&[3 << 3 | 3, 2]).is_ok());
        assert!(Packet::parse(&[16 << 3 | 3, 0]).is_err());
    }

    #[test]
    fn self_delimited() {
        // Code 0: the length precedes the frame; bytes after it are not its.
        let (p, used) = Packet::parse_self_delimited(&[8, 2, 1, 2, 99]).unwrap();
        assert_eq!((lengths(&p), used), (vec![2], 4));
        // Code 1: one length for both.
        let (p, used) = Packet::parse_self_delimited(&[8 | 1, 1, 1, 2, 99]).unwrap();
        assert_eq!((lengths(&p), used), (vec![1, 1], 4));
        // Code 2: N1 then the second frame's length.
        let (p, used) = Packet::parse_self_delimited(&[8 | 2, 1, 2, 7, 8, 9, 99]).unwrap();
        assert_eq!((lengths(&p), used), (vec![1, 2], 6));
        // CBR code 3 with one byte of padding.
        let data = [31 << 3 | 3, 0x40 | 2, 1, 1, 7, 8, 0, 99];
        let (p, used) = Packet::parse_self_delimited(&data).unwrap();
        assert_eq!((lengths(&p), used), (vec![1, 1], 7));
        // VBR code 3: N1, then the last frame's length.
        let data = [31 << 3 | 3, 0x80 | 2, 1, 2, 7, 8, 9, 99];
        let (p, used) = Packet::parse_self_delimited(&data).unwrap();
        assert_eq!((lengths(&p), used), (vec![1, 2], 7));
        assert!(Packet::parse_self_delimited(&[8, 5, 1]).is_err());
    }

    #[test]
    fn multistream() {
        // A self-delimited 20 ms packet, then an undelimited one.
        let data = [8, 1, 0xAA, 8 | 4, 0xBB, 0xCC];
        let mut seen = Vec::new();
        let samples = for_each_stream(&data, 2, |i, p| {
            seen.push((i, p.frames()[0].to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(samples, 960);
        assert_eq!(seen, [(0, vec![0xAA]), (1, vec![0xBB, 0xCC])]);
        // A 10 ms packet beside a 20 ms one.
        let mut visited = 0;
        assert!(
            for_each_stream(&[0, 1, 0xAA, 8, 0xBB], 2, |_, _| {
                visited += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(visited, 0);
        // A second stream cut short.
        assert!(for_each_stream(&[8, 1, 0xAA, 8 | 3], 2, |_, _| panic!("decoded")).is_err());
        assert!(for_each_stream(&[8], 0, |_, _| Ok(())).is_err());
    }
}
