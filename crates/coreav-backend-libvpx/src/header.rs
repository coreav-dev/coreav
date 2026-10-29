//! The frame sizes a VP9 packet states, read before libvpx sees the
//! packet (VP9 Bitstream and Decoding Process Specification 0.6, section
//! 6.2, "Uncompressed header syntax", and Annex B, "Superframes").
//!
//! A packet is one frame, or a superframe: frames one after another and an
//! index at the end. Each frame's uncompressed header states its size
//! (`frame_size()`) when it is a key frame, an intra-only frame or an
//! inter frame that does not take a reference frame's size. A frame that
//! shows a frame again (`show_existing_frame`) or takes a reference
//! frame's size (`found_ref`) states none: its size is one an earlier frame
//! stated. So checking every stated size against the decode limits bounds
//! every frame libvpx decodes.
#![forbid(unsafe_code)]

use std::fmt;

/// A frame's coded size, as its header states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    /// `frame_width_minus_1 + 1`.
    pub width: u32,
    /// `frame_height_minus_1 + 1`.
    pub height: u32,
}

/// A packet whose frame headers cannot be read: what was wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// The sizes the frames of `packet` state, in order (see the module's
/// description for the frames that state none).
pub fn frame_sizes(packet: &[u8]) -> Result<Vec<FrameSize>, Malformed> {
    let mut sizes = Vec::new();
    for frame in frames(packet)? {
        if let Some(size) = stated_size(frame)? {
            sizes.push(size);
        }
    }
    Ok(sizes)
}

/// The frames of a packet: those its superframe index lists (Annex B.3),
/// or the packet itself when its last byte and the byte the index would
/// start at are not the same marker.
fn frames(packet: &[u8]) -> Result<Vec<&[u8]>, Malformed> {
    let Some(&marker) = packet.last() else {
        return Err(Malformed("empty packet"));
    };
    if marker & 0xE0 != 0xC0 {
        return Ok(vec![packet]);
    }
    let count = usize::from(marker & 7) + 1;
    let width = usize::from((marker >> 3) & 3) + 1;
    let index = 2 + width * count;
    let Some(start) = packet.len().checked_sub(index) else {
        return Ok(vec![packet]);
    };
    if packet.get(start) != Some(&marker) {
        return Ok(vec![packet]);
    }
    let table = packet.get(start + 1..packet.len() - 1).unwrap_or_default();
    let mut rest = packet.get(..start).unwrap_or_default();
    let mut out = Vec::with_capacity(count);
    for entry in table.chunks_exact(width) {
        let size = entry
            .iter()
            .rev()
            .fold(0usize, |size, &byte| (size << 8) | usize::from(byte));
        if size == 0 {
            continue;
        }
        let Some((frame, tail)) = rest.split_at_checked(size) else {
            return Err(Malformed("superframe index larger than its frames"));
        };
        out.push(frame);
        rest = tail;
    }
    Ok(out)
}

/// The size a frame's uncompressed header states, if any.
fn stated_size(frame: &[u8]) -> Result<Option<FrameSize>, Malformed> {
    let mut bits = Bits::new(frame);
    if bits.read(2)? != 2 {
        return Err(Malformed("frame marker is not 2"));
    }
    let low = bits.read(1)?;
    let profile = (bits.read(1)? << 1) | low;
    if profile == 3 {
        bits.read(1)?;
    }
    if bits.read(1)? == 1 {
        return Ok(None);
    }
    let key_frame = bits.read(1)? == 0;
    let show_frame = bits.read(1)? == 1;
    let error_resilient = bits.read(1)? == 1;
    if key_frame {
        sync_code(&mut bits)?;
        color_config(&mut bits, profile)?;
        return frame_size(&mut bits).map(Some);
    }
    let intra_only = !show_frame && bits.read(1)? == 1;
    if !error_resilient {
        bits.read(2)?;
    }
    if intra_only {
        sync_code(&mut bits)?;
        if profile > 0 {
            color_config(&mut bits, profile)?;
        }
        bits.read(8)?;
        return frame_size(&mut bits).map(Some);
    }
    bits.read(8)?;
    bits.read(3 * 4)?;
    for _ in 0..3 {
        if bits.read(1)? == 1 {
            return Ok(None);
        }
    }
    frame_size(&mut bits).map(Some)
}

fn sync_code(bits: &mut Bits<'_>) -> Result<(), Malformed> {
    if bits.read(24)? == 0x49_83_42 {
        Ok(())
    } else {
        Err(Malformed("frame sync code is not 0x498342"))
    }
}

/// `color_config()`, read past.
fn color_config(bits: &mut Bits<'_>, profile: u32) -> Result<(), Malformed> {
    if profile >= 2 {
        bits.read(1)?;
    }
    let rgb = bits.read(3)? == 7;
    let subsampled = profile == 1 || profile == 3;
    if !rgb {
        bits.read(1)?;
    }
    match (rgb, subsampled) {
        (false, true) => bits.read(3).map(|_| ()),
        (true, true) => bits.read(1).map(|_| ()),
        (_, false) => Ok(()),
    }
}

fn frame_size(bits: &mut Bits<'_>) -> Result<FrameSize, Malformed> {
    let width = bits.read(16)? + 1;
    let height = bits.read(16)? + 1;
    Ok(FrameSize { width, height })
}

/// The bits of a header, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    /// The next `n` bits (at most 32) as an unsigned number.
    fn read(&mut self, n: u32) -> Result<u32, Malformed> {
        let mut value = 0u32;
        for _ in 0..n {
            let byte = self
                .data
                .get(self.at / 8)
                .ok_or(Malformed("frame header shorter than its fields"))?;
            value = (value << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes header fields most significant bit first.
    #[derive(Default)]
    struct Writer {
        bits: Vec<bool>,
    }

    impl Writer {
        fn put(mut self, n: u32, value: u32) -> Self {
            for i in (0..n).rev() {
                self.bits.push((value >> i) & 1 == 1);
            }
            self
        }

        fn bytes(&self) -> Vec<u8> {
            self.bits
                .chunks(8)
                .map(|c| {
                    c.iter()
                        .enumerate()
                        .fold(0u8, |b, (i, &bit)| b | (u8::from(bit) << (7 - i)))
                })
                .collect()
        }
    }

    /// The start of a frame header: marker, profile, no
    /// `show_existing_frame`, frame type, `show_frame`, error resilience.
    fn start(profile: u32, key: bool, show: bool, resilient: bool) -> Writer {
        let w = Writer::default()
            .put(2, 2)
            .put(1, profile & 1)
            .put(1, profile >> 1);
        let w = if profile == 3 { w.put(1, 0) } else { w };
        w.put(1, 0)
            .put(1, u32::from(!key))
            .put(1, u32::from(show))
            .put(1, u32::from(resilient))
    }

    fn key_frame(profile: u32, width: u32, height: u32) -> Vec<u8> {
        let w = start(profile, true, true, false).put(24, 0x49_83_42);
        let w = if profile >= 2 { w.put(1, 0) } else { w };
        let w = w.put(3, 2).put(1, 0);
        let w = if profile == 1 || profile == 3 {
            w.put(3, 0b100)
        } else {
            w
        };
        w.put(16, width - 1).put(16, height - 1).put(1, 0).bytes()
    }

    fn size(width: u32, height: u32) -> FrameSize {
        FrameSize { width, height }
    }

    #[test]
    fn a_key_frame_states_its_size_in_every_profile() {
        for profile in 0..4 {
            assert_eq!(
                frame_sizes(&key_frame(profile, 1920, 1080)),
                Ok(vec![size(1920, 1080)]),
                "profile {profile}"
            );
        }
        assert_eq!(
            frame_sizes(&key_frame(0, 65_536, 1)),
            Ok(vec![size(65_536, 1)])
        );
    }

    #[test]
    fn inter_and_intra_only_frames() {
        // An inter frame with no reference's size states its own.
        let explicit = start(0, false, true, false)
            .put(2, 0)
            .put(8, 1)
            .put(12, 0)
            .put(3, 0)
            .put(16, 639)
            .put(16, 359)
            .bytes();
        assert_eq!(frame_sizes(&explicit), Ok(vec![size(640, 360)]));
        // One taking its second reference's size states none.
        let found = start(0, false, true, true)
            .put(8, 1)
            .put(12, 0)
            .put(2, 0b01)
            .bytes();
        assert_eq!(frame_sizes(&found), Ok(vec![]));
        // Intra-only (not shown): profile 0 has no colour configuration,
        // profile 2 has one.
        let intra = |profile: u32| {
            let w = start(profile, false, false, false)
                .put(1, 1)
                .put(2, 0)
                .put(24, 0x49_83_42);
            let w = if profile > 0 {
                w.put(1, 1).put(3, 1).put(1, 0)
            } else {
                w
            };
            w.put(8, 0xFF).put(16, 99).put(16, 49).bytes()
        };
        assert_eq!(frame_sizes(&intra(0)), Ok(vec![size(100, 50)]));
        assert_eq!(frame_sizes(&intra(2)), Ok(vec![size(100, 50)]));
        // A frame shown again states none.
        let existing = Writer::default().put(2, 2).put(2, 0).put(1, 1).put(3, 4);
        assert_eq!(frame_sizes(&existing.bytes()), Ok(vec![]));
    }

    #[test]
    fn a_superframe_lists_each_frame() {
        let first = key_frame(0, 352, 288);
        let second = key_frame(0, 176, 144);
        for width in 1..=4usize {
            let marker = 0xC0 | (((width - 1) as u8) << 3) | 1;
            let mut packet = [first.clone(), second.clone()].concat();
            packet.push(marker);
            for frame in [&first, &second] {
                packet.extend_from_slice(&frame.len().to_le_bytes()[..width]);
            }
            packet.push(marker);
            assert_eq!(
                frame_sizes(&packet),
                Ok(vec![size(352, 288), size(176, 144)]),
                "{width}-byte sizes"
            );
            let last = packet.len() - 1 - width;
            packet[last] = packet[last].wrapping_add(1);
            assert_eq!(
                frame_sizes(&packet).err(),
                Some(Malformed("superframe index larger than its frames"))
            );
        }
        // A last byte that looks like a marker without its twin at the
        // index's start is part of a frame.
        let mut single = key_frame(0, 64, 64);
        single.push(0xC1);
        assert_eq!(frame_sizes(&single), Ok(vec![size(64, 64)]));
    }

    #[test]
    fn malformed_headers_are_errors() {
        assert_eq!(frame_sizes(&[]), Err(Malformed("empty packet")));
        assert_eq!(
            frame_sizes(&[0x00]),
            Err(Malformed("frame marker is not 2"))
        );
        let mut bad_sync = key_frame(0, 64, 64);
        bad_sync[1] ^= 0x10;
        assert_eq!(
            frame_sizes(&bad_sync),
            Err(Malformed("frame sync code is not 0x498342"))
        );
        let whole = key_frame(0, 64, 64);
        for len in 1..whole.len() - 1 {
            assert_eq!(
                frame_sizes(&whole[..len]),
                Err(Malformed("frame header shorter than its fields")),
                "{len} bytes"
            );
        }
    }
}
