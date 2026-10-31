//! The Opus encoder process (`docs/design/transcode.md`, "Opus audio";
//! policy record 0042): libopus in a process of its own, run by the sandbox
//! launcher ([`crate::exec`]) as the `coreav` program with [`ENCODE_ARG`]
//! first, and the parent's reader of what it writes back.
//!
//! The parent writes interleaved 32-bit little-endian float samples at
//! 48 kHz to the process's standard input and closes it at the end of the
//! stream. The process ([`encode_main`], with the `libopus` feature)
//! writes to its standard output:
//!
//! - a header: the 4 bytes `Opus`, then the pre-skip (the encoder's delay
//!   in samples) as 16 bits little-endian;
//! - one record per packet, in order: its length (1 to
//!   [`MAX_PACKET_BYTES`]) as 16 bits little-endian, then its bytes. Every
//!   packet codes [`FRAME_SAMPLES`] samples; the last input frame is padded
//!   with silence, and silent frames follow until the packets cover the
//!   pre-skip and every input sample.
//!
//! The process is untrusted: [`PacketReader`] checks the header, every
//! record's length and every packet's structure (RFC 6716 section 3) and
//! duration before a byte of it is used, and the end of the stream; the
//! caller checks the packet count against the samples it sent
//! ([`packets_for`]).

#![forbid(unsafe_code)]

use std::ffi::OsString;

/// The argument after which the `coreav` binary runs as the Opus encoder.
pub const ENCODE_ARG: &str = "__coreav_opus_encode";

/// Samples per channel of every packet: 20 ms at 48 kHz.
pub const FRAME_SAMPLES: u32 = 960;

/// The largest packet: libopus's recommended output buffer.
pub const MAX_PACKET_BYTES: usize = 4000;

/// The largest pre-skip: 80 ms at 48 kHz, more than libopus's delay.
pub const MAX_PRE_SKIP: u16 = 3840;

/// The lowest and the highest bit rate, in bits a second: the range
/// libopus codes usefully (6 to 510 kbit/s).
pub const MIN_BITRATE: u32 = 6_000;
/// See [`MIN_BITRATE`].
pub const MAX_BITRATE: u32 = 510_000;

const MAGIC: [u8; 4] = *b"Opus";

/// The arguments after [`ENCODE_ARG`]: the channels (1 or 2) and the bit
/// rate in bits a second.
pub fn encode_args(channels: u8, bitrate: u32) -> Vec<OsString> {
    vec![
        ENCODE_ARG.into(),
        channels.to_string().into(),
        bitrate.to_string().into(),
    ]
}

/// The packets that cover `pre_skip` samples and then `samples` samples
/// per channel.
pub fn packets_for(pre_skip: u16, samples: u64) -> u64 {
    samples
        .saturating_add(u64::from(pre_skip))
        .div_ceil(u64::from(FRAME_SAMPLES))
}

/// The samples at 48 kHz that `packet` decodes to, once its structure is
/// checked against RFC 6716 section 3.4 (requirements R1 to R7): a TOC
/// byte, frames of at most 1,275 bytes whose lengths fit the packet, and at
/// most 120 ms in all. Err says which requirement failed.
pub fn packet_samples(packet: &[u8]) -> Result<u32, &'static str> {
    const MAX_FRAME: usize = 1275;
    let Some((&toc, rest)) = packet.split_first() else {
        return Err("an empty packet (R1)");
    };
    // Table 2: the frame duration of each configuration, in samples.
    let config = toc >> 3;
    let frame: u32 = match config {
        0..=11 => match config % 4 {
            0 => 480,
            1 => 960,
            2 => 1920,
            _ => 2880,
        },
        12..=15 => 480 << (config % 2),
        _ => 120 << (config % 4),
    };
    let too_long = |n: usize| {
        if n > MAX_FRAME {
            Err("a frame of more than 1275 bytes (R2)")
        } else {
            Ok(())
        }
    };
    let frames = match toc & 3 {
        0 => {
            too_long(rest.len())?;
            1
        }
        1 => {
            if rest.len() % 2 != 0 {
                return Err("two equal frames of an odd length (R3)");
            }
            too_long(rest.len() / 2)?;
            2
        }
        2 => {
            let (first, after) = length(rest)?;
            let second = after
                .len()
                .checked_sub(first)
                .ok_or("the first frame runs past the packet (R4)")?;
            too_long(first)?;
            too_long(second)?;
            2
        }
        _ => {
            let Some((&count, mut after)) = rest.split_first() else {
                return Err("no frame count byte (R6, R7)");
            };
            let (vbr, padded, m) = (count & 0x80 != 0, count & 0x40 != 0, count & 0x3F);
            if m == 0 || u32::from(m) * frame > 5760 {
                return Err("no frame, or more than 120 ms (R5)");
            }
            let mut padding = 0usize;
            if padded {
                loop {
                    let Some((&p, next)) = after.split_first() else {
                        return Err("the padding length is cut short (R6, R7)");
                    };
                    after = next;
                    padding += if p == 255 { 254 } else { usize::from(p) };
                    if p != 255 {
                        break;
                    }
                }
            }
            if vbr {
                let mut sum = 0usize;
                for _ in 1..m {
                    let (n, next) = length(after)?;
                    too_long(n)?;
                    sum += n;
                    after = next;
                }
                let last = after
                    .len()
                    .checked_sub(padding)
                    .and_then(|r| r.checked_sub(sum))
                    .ok_or("the frames and padding run past the packet (R7)")?;
                too_long(last)?;
            } else {
                let data = after
                    .len()
                    .checked_sub(padding)
                    .ok_or("the padding runs past the packet (R6)")?;
                if data % usize::from(m) != 0 {
                    return Err("constant-size frames that do not divide the packet (R6)");
                }
                too_long(data / usize::from(m))?;
            }
            u32::from(m)
        }
    };
    Ok(frames * frame)
}

/// A frame length's one or two bytes (RFC 6716 section 3.2.1), and what
/// follows them.
fn length(bytes: &[u8]) -> Result<(usize, &[u8]), &'static str> {
    match *bytes {
        [] => Err("a frame length is missing (R4, R7)"),
        [b, ref after @ ..] if b < 252 => Ok((usize::from(b), after)),
        [b, b2, ref after @ ..] => Ok((usize::from(b2) * 4 + usize::from(b), after)),
        [_] => Err("a two-byte frame length is cut short (R4, R7)"),
    }
}

/// Why the encoder process's output was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The header is not `Opus` and a pre-skip of at most
    /// [`MAX_PRE_SKIP`].
    Header,
    /// A record's length is 0 or above [`MAX_PACKET_BYTES`].
    Length(usize),
    /// A packet's structure (RFC 6716 section 3.4), with the requirement.
    Packet(&'static str),
    /// A packet codes another duration than [`FRAME_SAMPLES`].
    Duration(u32),
    /// The stream ends inside the header or a record.
    Truncated,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header => write!(f, "the Opus encoder's header is invalid"),
            Self::Length(n) => write!(f, "the Opus encoder wrote a packet of {n} bytes"),
            Self::Packet(why) => write!(f, "the Opus encoder wrote an invalid packet: {why}"),
            Self::Duration(n) => write!(
                f,
                "the Opus encoder wrote a packet of {n} samples, not {FRAME_SAMPLES}"
            ),
            Self::Truncated => write!(f, "the Opus encoder's output ends inside a packet"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads the encoder process's output as it arrives, in pieces of any
/// size: checks it and hands each packet on, once whole and checked.
#[derive(Debug, Default)]
pub struct PacketReader {
    /// Bytes of the header or of the record being read.
    pending: Vec<u8>,
    pre_skip: Option<u16>,
    packets: u64,
}

impl PacketReader {
    /// A reader at the start of the output.
    pub fn new() -> Self {
        Self::default()
    }

    /// The pre-skip, once the header is read.
    pub fn pre_skip(&self) -> Option<u16> {
        self.pre_skip
    }

    /// Packets handed on so far.
    pub fn packets(&self) -> u64 {
        self.packets
    }

    /// Reads `bytes`, the next piece of the output, handing every packet
    /// completed in it to `packet`. An error of `packet` stops the reading
    /// and is returned as it is.
    pub fn push<E: From<ReadError>>(
        &mut self,
        mut bytes: &[u8],
        packet: &mut dyn FnMut(&[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        while !bytes.is_empty() {
            let want = match (self.pre_skip, &*self.pending) {
                (None, _) => 6,
                (Some(_), [a, b, ..]) => 2 + usize::from(u16::from_le_bytes([*a, *b])),
                (Some(_), _) => 2,
            };
            let take = want.saturating_sub(self.pending.len()).min(bytes.len());
            let (head, tail) = bytes.split_at(take);
            self.pending.extend_from_slice(head);
            bytes = tail;
            if self.pending.len() < want {
                continue;
            }
            match self.pre_skip {
                None => {
                    let pre_skip = match *self.pending {
                        [m0, m1, m2, m3, p0, p1] if [m0, m1, m2, m3] == MAGIC => {
                            u16::from_le_bytes([p0, p1])
                        }
                        _ => return Err(ReadError::Header.into()),
                    };
                    if pre_skip > MAX_PRE_SKIP {
                        return Err(ReadError::Header.into());
                    }
                    self.pre_skip = Some(pre_skip);
                }
                Some(_) if want > 2 => {
                    let data = self.pending.get(2..).unwrap_or_default();
                    let samples = packet_samples(data).map_err(ReadError::Packet)?;
                    if samples != FRAME_SAMPLES {
                        return Err(ReadError::Duration(samples).into());
                    }
                    packet(data)?;
                    self.packets += 1;
                }
                // The length is read; the record's bytes come next.
                Some(_) => {
                    let len = match *self.pending {
                        [a, b] => usize::from(u16::from_le_bytes([a, b])),
                        _ => 0,
                    };
                    if len == 0 || len > MAX_PACKET_BYTES {
                        return Err(ReadError::Length(len).into());
                    }
                    continue;
                }
            }
            self.pending.clear();
        }
        Ok(())
    }

    /// Ends the reading at the end of the output: Err when it ended inside
    /// the header or a record. Returns the pre-skip.
    pub fn finish(self) -> Result<u16, ReadError> {
        match self.pre_skip {
            Some(pre_skip) if self.pending.is_empty() => Ok(pre_skip),
            _ => Err(ReadError::Truncated),
        }
    }
}

/// Runs this process as the Opus encoder: `args` are those after
/// [`ENCODE_ARG`] ([`encode_args`]). Reads the samples from standard
/// input and writes the stream the module describes to standard output.
/// Exits 0 once every packet is written, else 1 after a line on standard
/// error.
#[cfg(feature = "libopus")]
pub fn encode_main(args: impl IntoIterator<Item = OsString>) -> std::process::ExitCode {
    use std::io::Write;
    let stdin = std::io::stdin().lock();
    let stdout = std::io::BufWriter::new(std::io::stdout().lock());
    match encode(args, stdin, stdout) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "coreav opus encoder: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// [`encode_main`] on `input` and `output`.
#[cfg(feature = "libopus")]
pub fn encode(
    args: impl IntoIterator<Item = OsString>,
    mut input: impl std::io::Read,
    mut output: impl std::io::Write,
) -> Result<(), String> {
    use coreav_backend_libopus::OpusEncoder;
    let mut args = args.into_iter();
    let mut number = |what: &str| -> Result<u32, String> {
        args.next()
            .and_then(|a| a.to_str()?.parse::<u32>().ok())
            .ok_or_else(|| format!("no {what}"))
    };
    let channels = number("channel count")?;
    let bitrate = number("bit rate")?;
    let channels = u8::try_from(channels)
        .ok()
        .filter(|c| matches!(c, 1 | 2))
        .ok_or_else(|| format!("{channels} channels: the encoder takes 1 or 2"))?;
    if !(MIN_BITRATE..=MAX_BITRATE).contains(&bitrate) {
        return Err(format!(
            "bit rate {bitrate} outside {MIN_BITRATE} to {MAX_BITRATE}"
        ));
    }
    let mut encoder = OpusEncoder::new(channels, bitrate)?;
    let pre_skip = u16::try_from(encoder.lookahead()?)
        .ok()
        .filter(|p| *p <= MAX_PRE_SKIP)
        .ok_or("libopus reports a delay above 80 ms")?;
    let io = |e: std::io::Error| e.to_string();
    output.write_all(&MAGIC).map_err(io)?;
    output.write_all(&pre_skip.to_le_bytes()).map_err(io)?;
    let floats = FRAME_SAMPLES as usize * usize::from(channels);
    let frame_bytes = floats * 4;
    let mut bytes = vec![0u8; frame_bytes];
    let mut frame = vec![0f32; floats];
    let mut packet = vec![0u8; MAX_PACKET_BYTES];
    let (mut samples, mut packets) = (0u64, 0u64);
    let mut emit = |frame: &[f32], packets: &mut u64| -> Result<(), String> {
        let len = encoder.encode(frame, &mut packet)?;
        let data = packet.get(..len).ok_or("libopus wrote past its buffer")?;
        let len = u16::try_from(len).map_err(|_| "a packet above 64 KiB")?;
        output.write_all(&len.to_le_bytes()).map_err(io)?;
        output.write_all(data).map_err(io)?;
        *packets += 1;
        Ok(())
    };
    loop {
        // A whole frame, or what is left at the end of the input.
        let mut filled = 0;
        while filled < frame_bytes {
            let rest = bytes.get_mut(filled..).unwrap_or_default();
            match input.read(rest) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        if filled % (4 * usize::from(channels)) != 0 {
            return Err("the input ends inside a sample frame".into());
        }
        if filled == 0 {
            break;
        }
        for (value, chunk) in frame.iter_mut().zip(bytes.chunks_exact(4)) {
            *value = match *chunk {
                [a, b, c, d] => f32::from_le_bytes([a, b, c, d]),
                _ => 0.0,
            };
        }
        // The last, partial frame is padded with silence.
        let valid = filled / 4;
        frame.iter_mut().skip(valid).for_each(|v| *v = 0.0);
        samples += (valid / usize::from(channels)) as u64;
        emit(&frame, &mut packets)?;
        if filled < frame_bytes {
            break;
        }
    }
    frame.iter_mut().for_each(|v| *v = 0.0);
    while packets < packets_for(pre_skip, samples) {
        emit(&frame, &mut packets)?;
    }
    output.flush().map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(bytes: &[u8], step: usize) -> Result<(u16, Vec<Vec<u8>>), ReadError> {
        let mut reader = PacketReader::new();
        let mut packets = Vec::new();
        for piece in bytes.chunks(step.max(1)) {
            reader.push::<ReadError>(piece, &mut |p| {
                packets.push(p.to_vec());
                Ok(())
            })?;
        }
        Ok((reader.finish()?, packets))
    }

    fn stream(pre_skip: u16, packets: &[&[u8]]) -> Vec<u8> {
        let mut out = b"Opus".to_vec();
        out.extend_from_slice(&pre_skip.to_le_bytes());
        for p in packets {
            out.extend_from_slice(&u16::try_from(p.len()).unwrap().to_le_bytes());
            out.extend_from_slice(p);
        }
        out
    }

    #[test]
    fn packet_durations_follow_the_toc() {
        // CELT FB 20 ms, code 0.
        assert_eq!(packet_samples(&[31 << 3, 1, 2, 3]), Ok(960));
        // SILK NB 60 ms.
        assert_eq!(packet_samples(&[3 << 3]), Ok(2880));
        // Two equal 10 ms hybrid frames.
        assert_eq!(packet_samples(&[(12 << 3) | 1, 7, 7]), Ok(960));
        assert!(packet_samples(&[(12 << 3) | 1, 7]).is_err());
        // Two frames, the first of 1 byte.
        assert_eq!(packet_samples(&[(30 << 3) | 2, 1, 9, 9, 9]), Ok(960));
        assert!(packet_samples(&[(30 << 3) | 2, 5, 9]).is_err());
        assert!(packet_samples(&[(30 << 3) | 2, 252]).is_err());
        // Code 3: four CBR 5 ms frames of 2 bytes, 3 bytes of padding.
        assert_eq!(
            packet_samples(&[(29 << 3) | 3, 0x40 | 4, 3, 1, 1, 2, 2, 3, 3, 4, 4, 0, 0, 0]),
            Ok(960)
        );
        // More than 120 ms.
        assert!(packet_samples(&[(3 << 3) | 3, 3]).is_err());
        assert!(packet_samples(&[(3 << 3) | 3, 0]).is_err());
        // VBR: two 10 ms frames, lengths 1 and the rest.
        assert_eq!(
            packet_samples(&[(30 << 3) | 3, 0x80 | 2, 1, 5, 6, 6]),
            Ok(960)
        );
        assert!(packet_samples(&[(30 << 3) | 3, 0x80 | 2, 9, 5]).is_err());
        assert!(packet_samples(&[]).is_err());
        let long = vec![0u8; 1277];
        assert!(packet_samples(&long).is_err());
        assert_eq!(packet_samples(&long[..1276]), Ok(480));
    }

    #[test]
    fn reader_checks_and_splits_the_stream() {
        let a: &[u8] = &[31 << 3, 1, 2];
        let b: &[u8] = &[31 << 3];
        let bytes = stream(312, &[a, b]);
        for step in [1, 2, 3, 5, 1000] {
            let (pre_skip, packets) = read_all(&bytes, step).unwrap();
            assert_eq!(pre_skip, 312);
            assert_eq!(packets, vec![a.to_vec(), b.to_vec()]);
        }
        assert_eq!(
            read_all(&bytes[..bytes.len() - 1], 4),
            Err(ReadError::Truncated)
        );
        assert_eq!(read_all(b"Opu", 4), Err(ReadError::Truncated));
        assert_eq!(read_all(b"Opis\0\0", 4), Err(ReadError::Header));
        assert_eq!(read_all(&stream(3841, &[]), 4), Err(ReadError::Header));
        let mut zero = stream(0, &[]);
        zero.extend_from_slice(&[0, 0]);
        assert_eq!(read_all(&zero, 4), Err(ReadError::Length(0)));
        assert_eq!(
            read_all(&stream(0, &[&[3 << 3]]), 4),
            Err(ReadError::Duration(2880))
        );
    }

    #[test]
    fn packets_cover_the_pre_skip_and_every_sample() {
        assert_eq!(packets_for(312, 96_000), 101);
        assert_eq!(packets_for(312, 960 - 312), 1);
        assert_eq!(packets_for(312, 960 - 311), 2);
        assert_eq!(packets_for(0, 0), 0);
    }

    #[cfg(feature = "libopus")]
    #[test]
    fn the_encoder_writes_what_the_reader_takes() {
        let samples = 48_000 + 123;
        let mut input = Vec::new();
        for n in 0..samples {
            let v = (n as f32 * 0.05).sin() * 0.25;
            input.extend_from_slice(&v.to_le_bytes());
            input.extend_from_slice(&(-v).to_le_bytes());
        }
        let mut output = Vec::new();
        encode(
            encode_args(2, 96_000).into_iter().skip(1),
            &input[..],
            &mut output,
        )
        .unwrap();
        let (pre_skip, packets) = read_all(&output, 777).unwrap();
        assert!(pre_skip > 0);
        assert_eq!(packets.len() as u64, packets_for(pre_skip, samples as u64));
        assert!(
            encode(
                encode_args(3, 96_000).into_iter().skip(1),
                &input[..],
                Vec::new()
            )
            .is_err()
        );
        assert!(
            encode(
                encode_args(2, 96_000).into_iter().skip(1),
                &input[..5],
                Vec::new()
            )
            .is_err()
        );
    }
}
