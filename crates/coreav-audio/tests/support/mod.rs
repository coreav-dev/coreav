//! Test helpers: MD5 (RFC 1321) for the FLAC streaminfo checksum, and a
//! walker over raw FLAC files for the conformance vectors.
#![allow(dead_code)]

use coreav_audio::flac::{FlacDecoder, StreamInfo};
use coreav_audio::{Error, Limits, Pcm, Plane};

/// MD5 of `data` (RFC 1321).
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut state = Md5::new();
    state.update(data);
    state.finish()
}

/// Incremental MD5.
pub struct Md5 {
    h: [u32; 4],
    buffer: Vec<u8>,
    len: u64,
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            h: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buffer: Vec::new(),
            len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.len += data.len() as u64;
        self.buffer.extend_from_slice(data);
        let whole = self.buffer.len() / 64 * 64;
        let blocks: Vec<u8> = self.buffer.drain(..whole).collect();
        for block in blocks.chunks_exact(64) {
            self.block(block);
        }
    }

    pub fn finish(mut self) -> [u8; 16] {
        let bits = self.len.wrapping_mul(8);
        let mut tail = vec![0x80u8];
        while (self.buffer.len() + tail.len()) % 64 != 56 {
            tail.push(0);
        }
        tail.extend_from_slice(&bits.to_le_bytes());
        let len = self.len;
        self.update(&tail);
        self.len = len;
        let mut out = [0u8; 16];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn block(&mut self, block: &[u8]) {
        const S: [u32; 64] = [
            7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
            5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
            6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
        ];
        let k: Vec<u32> = (0..64)
            .map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32)
            .collect();
        let m: Vec<u32> = block
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = self.h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
    }
}

/// Appends the samples of `pcm` interleaved, signed little-endian, each
/// sign-extended to whole bytes of `bits` (RFC 9639 8.2).
pub fn interleaved_le(pcm: &Pcm, bits: u32, out: &mut Vec<u8>) {
    let bytes = bits.div_ceil(8) as usize;
    let planes: Vec<&[i32]> = (0..pcm.channels())
        .map(|c| match pcm.plane(c).unwrap() {
            Plane::Int { samples, .. } => samples,
            Plane::Float(_) => panic!("float samples"),
        })
        .collect();
    for i in 0..pcm.frames() {
        for plane in &planes {
            out.extend_from_slice(&plane[i].to_le_bytes()[..bytes]);
        }
    }
}

/// What decoding a raw FLAC file gave.
pub struct Decoded {
    pub info: StreamInfo,
    pub md5: [u8; 16],
    pub samples: u64,
    pub frames: u64,
    pub crc_failures: u64,
    /// The first samples, interleaved, at most 64.
    pub first: Vec<i32>,
}

/// Decodes a raw FLAC file (the `fLaC` signature, metadata blocks, then
/// frames back to back), frame by frame with the decoder's frame length.
pub fn decode_raw(file: &[u8]) -> Result<Decoded, Error> {
    let mut at = 4;
    assert_eq!(&file[..4], b"fLaC");
    let info = StreamInfo::from_metadata_blocks(&file[4..])?;
    loop {
        let header = &file[at..at + 4];
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        at += 4 + len;
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    let mut decoder = FlacDecoder::new(info, &Limits::default())?;
    let mut pcm = Pcm::new();
    let mut md5 = Md5::new();
    let mut bytes = Vec::new();
    let (mut samples, mut frames, mut crc_failures) = (0u64, 0u64, 0u64);
    let mut first = Vec::new();
    while at < file.len() {
        let frame = decoder.decode(&file[at..], &mut pcm)?;
        at += frame.bytes;
        frames += 1;
        crc_failures += u64::from(!frame.crc_ok);
        samples += u64::from(frame.block_size);
        bytes.clear();
        interleaved_le(&pcm, u32::from(info.bits_per_sample), &mut bytes);
        md5.update(&bytes);
        if first.len() < 64 {
            for i in 0..pcm.frames() {
                for c in 0..pcm.channels() {
                    if let Some(Plane::Int { samples, .. }) = pcm.plane(c) {
                        first.push(samples[i]);
                    }
                }
            }
            first.truncate(64);
        }
    }
    Ok(Decoded {
        info,
        md5: md5.finish(),
        samples,
        frames,
        crc_failures,
        first,
    })
}

/// Bytes of an `xxd`-style dump as printed in RFC 9639 Appendix D.
pub fn from_dump(dump: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in dump.lines() {
        let Some((_, rest)) = line.split_once(": ") else {
            continue;
        };
        let hex: String = rest
            .split("  ")
            .next()
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        for i in (0..hex.len()).step_by(2) {
            out.push(u8::from_str_radix(&hex[i..i + 2], 16).unwrap());
        }
    }
    out
}
