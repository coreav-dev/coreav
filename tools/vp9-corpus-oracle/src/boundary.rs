use std::time::Duration;

use coreav_backend_vp9::{NativeOptions, NativeVp9};
use coreav_decode::{Backend, Codec, Decoder, Limits, Packet, StreamConfig};
use coreav_frame::{PixelFormat, Pixels};
use coreav_vp9::{Header, Picture};
use md5::{Digest, Md5};

use crate::Scope;

type Row = (Option<i64>, u32, u32, u64, String);

pub(super) struct Boundary {
    full: Box<dyn Decoder>,
    key: Box<dyn Decoder>,
    key_slots: [bool; 8],
}

impl Boundary {
    pub(super) fn new(scope: Scope) -> Result<Self, String> {
        let codec_limits = coreav_vp9::Limits::default();
        let limits = Limits {
            max_width: codec_limits.max_width,
            max_height: codec_limits.max_height,
            max_pixels: codec_limits.max_pixels,
            max_packet_bytes: codec_limits.max_packet_bytes as u64,
            max_decoded_bytes: u64::MAX,
            timeout: Duration::from_secs(if scope == Scope::All { 3600 } else { 900 }),
            ..Limits::default()
        };
        let stream = StreamConfig::new(Codec::Vp9);
        Ok(Self {
            full: NativeVp9::new()
                .open(&stream, &limits)
                .map_err(|e| e.to_string())?,
            key: NativeVp9::with_options(NativeOptions {
                keyframes_only: true,
            })
            .open(&stream, &limits)
            .map_err(|e| e.to_string())?,
            key_slots: [false; 8],
        })
    }

    pub(super) fn check(
        &mut self,
        bytes: &[u8],
        pts: i64,
        headers: &[Header],
        pictures: &[Picture],
    ) -> Result<(), String> {
        let packet = Packet {
            data: bytes,
            pts: Some(pts),
            dts: None,
            keyframe: false,
            discard: false,
        };
        self.full.send_packet(&packet).map_err(|e| e.to_string())?;
        self.key.send_packet(&packet).map_err(|e| e.to_string())?;
        let mut selected = Vec::new();
        for header in headers {
            match header {
                Header::ShowExisting { slot, .. } => selected.push(
                    *self
                        .key_slots
                        .get(usize::from(*slot))
                        .ok_or("invalid shown slot")?,
                ),
                Header::Frame { header, .. } => {
                    for (slot, key) in self.key_slots.iter_mut().enumerate() {
                        if header.refresh_frame_flags & (1 << slot) != 0 {
                            *key = header.key_frame;
                        }
                    }
                    if header.show_frame {
                        selected.push(header.key_frame);
                    }
                }
            }
        }
        if selected.len() != pictures.len() {
            return Err("header and full frame counts disagree".into());
        }
        let expected = pictures
            .iter()
            .map(|picture| {
                let (size, md5) = crate::packed(picture)?;
                Ok((Some(pts), picture.width, picture.height, size, md5))
            })
            .collect::<Result<Vec<Row>, String>>()?;
        if drain(self.full.as_mut())? != expected {
            return Err("full VP9 boundary output disagrees with scalar output".into());
        }
        let expected_keys = expected
            .into_iter()
            .zip(selected)
            .filter_map(|(row, key)| key.then_some(row))
            .collect::<Vec<_>>();
        if drain(self.key.as_mut())? != expected_keys {
            return Err("keyframe-only VP9 boundary output disagrees with full output".into());
        }
        Ok(())
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        for decoder in [&mut self.full, &mut self.key] {
            decoder.send_eof().map_err(|e| e.to_string())?;
            if !drain(decoder.as_mut())?.is_empty() {
                return Err("unexpected delayed VP9 boundary output".into());
            }
        }
        Ok(())
    }
}

fn drain(decoder: &mut dyn Decoder) -> Result<Vec<Row>, String> {
    let mut rows = Vec::new();
    while let Some(frame) = decoder.receive_frame().map_err(|e| e.to_string())? {
        let Pixels::Cpu(picture) = frame.pixels else {
            return Err("native VP9 frame is not in CPU memory".into());
        };
        if picture.format() != PixelFormat::YUV420P {
            return Err("native VP9 frame has the wrong pixel format".into());
        }
        let mut md5 = Md5::new();
        for plane in 0..picture.planes().len() {
            for row in picture.rows(plane) {
                md5.update(row);
            }
        }
        rows.push((
            frame.pts,
            picture.width(),
            picture.height(),
            picture.packed_len() as u64,
            format!("{:x}", md5.finalize()),
        ));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 15] = [
        0x82, 0x49, 0x83, 0x42, 0x40, 0x03, 0xf0, 0x02, 0xf5, 0x80, 0x07, 0x80, 0x00, 0x01, 0x00,
    ];

    #[test]
    fn boundary_comparisons_reject_changed_pixels_counts_and_key_selection() {
        for mutation in 0..4 {
            let limits = coreav_vp9::Limits::default();
            let mut headers = coreav_vp9::Decoder::new(limits).read_headers(&KEY).unwrap();
            let mut pictures = coreav_vp9::FrameDecoder::new(limits).decode(&KEY).unwrap();
            match mutation {
                1 => {
                    if let Some(coreav_vp9::Samples::Eight(y)) =
                        pictures.first_mut().and_then(|p| p.planes.first_mut())
                    {
                        let first = y.first_mut().unwrap();
                        *first ^= 1;
                    } else {
                        panic!("profile-0 luma");
                    }
                }
                2 => pictures.clear(),
                3 => {
                    let Some(Header::Frame { header, .. }) = headers.first_mut() else {
                        panic!("coded keyframe");
                    };
                    header.key_frame = false;
                }
                _ => {}
            }
            let mut boundary = Boundary::new(Scope::First120).unwrap();
            let result = boundary.check(&KEY, -17, &headers, &pictures);
            assert_eq!(result.is_ok(), mutation == 0);
            if mutation == 0 {
                boundary.finish().unwrap();
            }
        }
    }
}
