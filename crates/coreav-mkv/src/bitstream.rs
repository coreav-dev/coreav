//! Frame header fields for codecs whose track header does not carry them:
//! the VP9 uncompressed header (VP9 bitstream specification 6.2), the VP8
//! frame tag (RFC 6386 9.1), and the parameter sets some writers keep only
//! in the first keyframe (an H.264 sequence parameter set, ITU-T H.264
//! 7.3.1 and Annex B; an AV1 sequence header OBU, AV1 specification 5.3).
//! These are header reads, never decoding.

/// What the first frame of a VP8 or VP9 track says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FrameInfo {
    /// VP9 `Profile`, or the VP8 `version`.
    pub profile: u8,
    /// VP9 `BitDepth`.
    pub bit_depth: Option<u8>,
    /// VP9 `color_space` as an ITU-T H.273 matrix code point.
    pub matrix: Option<u16>,
    /// VP9 `color_range`.
    pub full_range: Option<bool>,
    /// VP9 `subsampling_x` and `subsampling_y` (1 = halved).
    pub subsampling: Option<(u8, u8)>,
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bit(&mut self) -> Option<u8> {
        let byte = self.data.get(self.at / 8)?;
        let bit = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Some(bit)
    }

    fn bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u32::from(self.bit()?);
        }
        Some(value)
    }
}

/// The VP9 uncompressed header of the first frame in `frame`: profile,
/// and for a key frame the color configuration (6.2, `color_config`).
/// `None` when the bytes are not a VP9 frame header.
pub(crate) fn vp9(frame: &[u8]) -> Option<FrameInfo> {
    let mut b = Bits { data: frame, at: 0 };
    if b.bits(2)? != 2 {
        return None;
    }
    let low = b.bit()?;
    let high = b.bit()?;
    let profile = (high << 1) | low;
    if profile == 3 {
        b.bit()?;
    }
    let mut info = FrameInfo {
        profile,
        ..FrameInfo::default()
    };
    if b.bit()? == 1 {
        // show_existing_frame: no more header.
        return Some(info);
    }
    let key_frame = b.bit()? == 0;
    b.bits(2)?;
    if !key_frame {
        return Some(info);
    }
    if b.bits(24)? != 0x49_8342 {
        return None;
    }
    info.bit_depth = Some(if profile >= 2 {
        if b.bit()? == 1 { 12 } else { 10 }
    } else {
        8
    });
    let color_space = b.bits(3)?;
    // CS_UNKNOWN, CS_BT_601, CS_BT_709, CS_SMPTE_170, CS_SMPTE_240,
    // CS_BT_2020, CS_RESERVED, CS_RGB as H.273 matrix coefficients.
    info.matrix = match color_space {
        1 => Some(5),
        2 => Some(1),
        3 => Some(6),
        4 => Some(7),
        5 => Some(9),
        7 => Some(0),
        _ => None,
    };
    let rgb = color_space == 7;
    info.full_range = Some(if rgb { true } else { b.bit()? == 1 });
    // color_config: profiles 1 and 3 store the subsampling (RGB is 4:4:4);
    // profiles 0 and 2 are 4:2:0.
    info.subsampling = Some(match (profile, rgb) {
        (1 | 3, false) => {
            let x = b.bit()?;
            let y = b.bit()?;
            (x, y)
        }
        (1 | 3, true) => (0, 0),
        _ => (1, 1),
    });
    Some(info)
}

/// The VP8 frame tag (RFC 6386 9.1): the `version` of a key frame, whose
/// start code follows the tag. `None` for anything else.
pub(crate) fn vp8(frame: &[u8]) -> Option<FrameInfo> {
    let [tag, _, _, 0x9d, 0x01, 0x2a, ..] = *frame else {
        return None;
    };
    if tag & 1 != 0 {
        return None;
    }
    Some(FrameInfo {
        profile: (tag >> 1) & 7,
        ..FrameInfo::default()
    })
}

/// A configuration record built from the parameter set in the first
/// keyframe, in the layout the ISO base media readers take: an `avcC`
/// holding the first H.264 sequence parameter set (Annex B start codes or
/// 4-byte lengths), or the stored `av1C` header followed by the first AV1
/// sequence header OBU.
pub(crate) fn h264_record(frame: &[u8]) -> Option<Vec<u8>> {
    let sps = nal_units(frame).find(|nal| nal.first().is_some_and(|h| h & 0x1f == 7))?;
    let [_, profile, compatibility, level, ..] = *sps else {
        return None;
    };
    let len = u16::try_from(sps.len()).ok()?.to_be_bytes();
    let mut record = vec![1, profile, compatibility, level, 0xFF, 0xE1];
    record.extend_from_slice(&len);
    record.extend_from_slice(sps);
    record.push(0);
    Some(record)
}

/// The NAL units of an H.264 access unit: split at Annex B start codes when
/// it starts with one, else by 4-byte big-endian lengths.
fn nal_units(frame: &[u8]) -> Box<dyn Iterator<Item = &[u8]> + '_> {
    if frame.starts_with(&[0, 0, 1]) || frame.starts_with(&[0, 0, 0, 1]) {
        let mut rest = frame;
        Box::new(core::iter::from_fn(move || {
            let start = start_code_end(rest)?;
            let body = rest.get(start..)?;
            let end = (0..body.len())
                .find(|&i| body.get(i..).is_some_and(|b| b.starts_with(&[0, 0, 1])))
                .unwrap_or(body.len());
            let nal = body.get(..end)?;
            rest = body.get(end..)?;
            // Trailing zero bytes before a 4-byte start code belong to it.
            Some(nal.strip_suffix(&[0]).unwrap_or(nal))
        }))
    } else {
        let mut rest = frame;
        Box::new(core::iter::from_fn(move || {
            let len = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?);
            let end = 4usize.checked_add(usize::try_from(len).ok()?)?;
            let nal = rest.get(4..end)?;
            rest = rest.get(end..)?;
            Some(nal)
        }))
    }
}

fn start_code_end(bytes: &[u8]) -> Option<usize> {
    if bytes.starts_with(&[0, 0, 1]) {
        Some(3)
    } else if bytes.starts_with(&[0, 0, 0, 1]) {
        Some(4)
    } else {
        None
    }
}

/// See [`h264_record`]: `header` is the stored `av1C` (at least its 4-byte
/// header); the frame's OBUs follow it as configuration OBUs, which the
/// `av1C` reader walks to the first sequence header.
pub(crate) fn av1_record(header: &[u8], frame: &[u8]) -> Option<Vec<u8>> {
    let mut record = header.get(..4)?.to_vec();
    record.extend_from_slice(frame);
    Some(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vp9_key_frame_headers() {
        // frame_marker 2, profile 0, show_existing 0, key frame, show 1,
        // error resilient 0, sync code, color_space 2 (BT.709), range 1.
        let header = [0b1000_0010, 0x49, 0x83, 0x42, 0b0101_0000];
        assert_eq!(
            vp9(&header),
            Some(FrameInfo {
                profile: 0,
                bit_depth: Some(8),
                matrix: Some(1),
                full_range: Some(true),
                subsampling: Some((1, 1)),
            })
        );
        // Profile 2 (high bit set), 10-bit, CS_UNKNOWN, studio range.
        let header = [0b1001_0010, 0x49, 0x83, 0x42, 0b0000_0000];
        let info = vp9(&header).unwrap();
        assert_eq!(
            (info.profile, info.bit_depth, info.matrix),
            (2, Some(10), None)
        );
        assert_eq!(info.full_range, Some(false));
        // A non-key frame has no color configuration.
        assert_eq!(vp9(&[0b1000_0110]).unwrap().full_range, None);
        assert_eq!(vp9(&[0b0100_0000]), None);
        assert_eq!(vp9(&[]), None);
    }

    #[test]
    fn parameter_sets_from_frames() {
        let sps = [0x67, 0x42, 0xc0, 0x20, 0x8c];
        let annex_b = [
            &[0, 0, 0, 1][..],
            &[0x09, 0xf0],
            &[0, 0, 1],
            &sps,
            &[0, 0, 0, 1, 0x68, 0xce],
        ]
        .concat();
        let record = h264_record(&annex_b).unwrap();
        assert_eq!(&record[..8], &[1, 0x42, 0xc0, 0x20, 0xFF, 0xE1, 0, 5]);
        assert_eq!(&record[8..13], &sps);
        let lengths = [&[0, 0, 0, 2][..], &[0x09, 0xf0], &[0, 0, 0, 5], &sps].concat();
        assert_eq!(h264_record(&lengths).unwrap()[8..13], sps);
        assert_eq!(h264_record(&[0, 0, 1, 0x65, 0x88]), None);
        // The frame's OBUs follow the stored av1C header.
        let frame = [0x12, 0x00, 0x0a, 0x03, 0xaa, 0xbb, 0xcc];
        let record = av1_record(&[0x81, 0x09, 0x0c, 0x00, 0x99], &frame).unwrap();
        assert_eq!(record, [&[0x81, 0x09, 0x0c, 0x00][..], &frame].concat());
        assert_eq!(av1_record(&[0x81], &frame), None);
    }

    #[test]
    fn vp8_frame_tags() {
        assert_eq!(
            vp8(&[0x50, 0x2a, 0x00, 0x9d, 0x01, 0x2a]).unwrap().profile,
            0
        );
        assert_eq!(
            vp8(&[0x52, 0x2a, 0x00, 0x9d, 0x01, 0x2a]).unwrap().profile,
            1
        );
        assert_eq!(vp8(&[0x51, 0x2a, 0x00, 0x9d, 0x01, 0x2a]), None);
        assert_eq!(vp8(&[0x50, 0x2a]), None);
    }
}
