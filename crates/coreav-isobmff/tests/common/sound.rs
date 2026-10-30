//! QuickTime compressed audio whose sample tables count single audio
//! samples: `stsz` of 1 and a version 1 sound description giving the
//! samples and bytes of one compressed frame (QuickTime File Format, sound
//! sample descriptions).

use super::{bx, cat, full, u32s};

pub const PER_PACKET: u32 = 2048;
pub const FRAME_BYTES: u32 = 370;

/// A version 1 sound description of `QDM2`: 2048 samples in 370 bytes.
fn qdm2_entry() -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&1u16.to_be_bytes()); // version 1
    body.extend_from_slice(&[0u8; 6]);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&(-2i16).to_be_bytes());
    body.extend_from_slice(&[0u8; 2]);
    body.extend_from_slice(&(44_100u32 << 16).to_be_bytes());
    body.extend_from_slice(&u32s(&[PER_PACKET, FRAME_BYTES / 2, FRAME_BYTES, 2]));
    bx(b"QDM2", &body)
}

/// A movie whose one audio track has a chunk of `samples` audio samples
/// for each entry of `chunks`, the movie box first and the frames of the
/// chunks back to back in `mdat` at the end, short of `cut` bytes.
pub fn qdm2_movie(chunks: &[u32], cut: usize) -> Vec<u8> {
    let frames: Vec<u32> = chunks.iter().map(|n| n.div_ceil(PER_PACKET)).collect();
    let samples: u32 = chunks.iter().sum();
    let ftyp = bx(b"ftyp", b"qt  \0\0\x02\0qt  ");
    let build = |first: u32| {
        let mut offsets = Vec::new();
        let mut at = first;
        for f in &frames {
            offsets.push(at);
            at += f * FRAME_BYTES;
        }
        let mut mvhd = u32s(&[0, 0, 44_100, samples]);
        mvhd.extend_from_slice(&[0u8; 80]);
        let mut tkhd = u32s(&[0, 0, 1, 0, samples]);
        tkhd.extend_from_slice(&[0u8; 52]);
        let mut mdhd = u32s(&[0, 0, 44_100, samples]);
        mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
        let mut hdlr = u32s(&[0]);
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 13]);
        let stsc: Vec<u32> = chunks
            .iter()
            .enumerate()
            .flat_map(|(i, n)| [i as u32 + 1, *n, 1])
            .collect();
        let stbl = bx(
            b"stbl",
            &cat(&[
                full(b"stsd", 0, 0, &cat(&[u32s(&[1]), qdm2_entry()])),
                full(b"stts", 0, 0, &u32s(&[1, samples, 1])),
                full(
                    b"stsc",
                    0,
                    0,
                    &cat(&[u32s(&[chunks.len() as u32]), u32s(&stsc)]),
                ),
                full(b"stsz", 0, 0, &u32s(&[1, samples])),
                full(
                    b"stco",
                    0,
                    0,
                    &cat(&[u32s(&[offsets.len() as u32]), u32s(&offsets)]),
                ),
            ]),
        );
        let dref = full(b"dref", 0, 0, &cat(&[u32s(&[1]), full(b"url ", 0, 1, &[])]));
        let minf = bx(
            b"minf",
            &cat(&[full(b"smhd", 0, 0, &[0u8; 4]), bx(b"dinf", &dref), stbl]),
        );
        let mdia = bx(
            b"mdia",
            &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
        );
        bx(
            b"moov",
            &cat(&[
                full(b"mvhd", 0, 0, &mvhd),
                bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia])),
            ]),
        )
    };
    // The offsets have a fixed width, so the movie box's length does not
    // depend on them.
    let first = (ftyp.len() + build(0).len() + 8) as u32;
    let total = (frames.iter().sum::<u32>() * FRAME_BYTES) as usize - cut;
    cat(&[ftyp, build(first), bx(b"mdat", &vec![0xAB; total])])
}
