//! Hosted checks of profile-0 pixels, packet clocks and keyframe selection.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::time::Duration;

use coreav_backend_vp9::{NativeOptions, NativeVp9};
use coreav_decode::{Backend, Codec, Decoder, Limits, Packet, StreamConfig};
use coreav_frame::{Frame, Pixels};
use coreav_vp9::Header;
use md5::{Digest, Md5};

#[path = "../../coreav-vp9/tests/common/mod.rs"]
mod common;

fn signature(frame: &Frame) -> (Option<i64>, u32, u32, String) {
    let Pixels::Cpu(picture) = &frame.pixels else {
        panic!("native frames must be in CPU memory");
    };
    let mut md5 = Md5::new();
    for plane in 0..picture.planes().len() {
        for row in picture.rows(plane) {
            md5.update(row);
        }
    }
    (
        frame.pts,
        picture.width(),
        picture.height(),
        format!("{:x}", md5.finalize()),
    )
}

fn drain(decoder: &mut dyn Decoder) -> Vec<Frame> {
    let mut frames = Vec::new();
    while let Some(frame) = decoder.receive_frame().unwrap() {
        frames.push(frame);
    }
    frames
}

#[test]
#[ignore = "needs checksum-verified vectors in COREAV_VP9_VECTORS"]
fn profile_zero_boundary_pixels_clocks_and_keyframe_selection_agree() {
    let dir = PathBuf::from(std::env::var_os("COREAV_VP9_VECTORS").expect("COREAV_VP9_VECTORS"));
    let limits = Limits {
        max_width: 1 << 16,
        max_height: 1 << 16,
        max_pixels: 1 << 32,
        max_decoded_bytes: 64 << 30,
        max_pending_bytes: 1 << 30,
        timeout: Duration::from_secs(300),
        ..Limits::default()
    };
    let mut vectors = 0;
    let mut total = 0;
    let mut keys = 0;
    for (name, _) in common::manifest()
        .into_iter()
        .filter(|(name, _)| name.starts_with("vp90-"))
    {
        let data = std::fs::read(dir.join(&name)).unwrap();
        let list = std::fs::read_to_string(dir.join(format!("{name}.md5"))).unwrap();
        let published: Vec<&str> = list
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .collect();
        let packets = if data.starts_with(b"DKIF") {
            common::ivf_packets(&data)
        } else {
            common::webm_packets(&data)
        };
        let stream = StreamConfig::new(Codec::Vp9);
        let mut full = NativeVp9::new().open(&stream, &limits).unwrap();
        let mut key = NativeVp9::with_options(NativeOptions {
            keyframes_only: true,
        })
        .open(&stream, &limits)
        .unwrap();
        let mut headers = coreav_vp9::Decoder::new(coreav_vp9::Limits {
            max_width: limits.max_width,
            max_height: limits.max_height,
            max_pixels: limits.max_pixels,
            ..coreav_vp9::Limits::default()
        });
        let mut key_slots = [false; 8];
        let mut shown = 0;
        for (index, data) in packets.into_iter().enumerate() {
            let pts = i64::try_from(index).unwrap() * 37 - 91;
            let packet = Packet {
                data,
                pts: Some(pts),
                dts: None,
                keyframe: false,
                discard: false,
            };
            full.send_packet(&packet)
                .unwrap_or_else(|error| panic!("{name}, packet {index}: {error}"));
            key.send_packet(&packet)
                .unwrap_or_else(|error| panic!("{name}, key packet {index}: {error}"));
            let full_frames = drain(full.as_mut());
            let key_frames = drain(key.as_mut());
            let mut expected = Vec::new();
            let mut frame = 0;
            for header in headers.read_headers(data).unwrap() {
                let (visible, is_key, width, height) = match header {
                    Header::ShowExisting {
                        slot,
                        width,
                        height,
                        ..
                    } => (true, key_slots[usize::from(slot)], width, height),
                    Header::Frame { header, .. } => {
                        for (slot, is_key) in key_slots.iter_mut().enumerate() {
                            if header.refresh_frame_flags & (1 << slot) != 0 {
                                *is_key = header.key_frame;
                            }
                        }
                        (
                            header.show_frame,
                            header.key_frame,
                            header.width,
                            header.height,
                        )
                    }
                };
                if visible {
                    let signature = signature(full_frames.get(frame).expect("every shown member"));
                    assert_eq!(
                        (signature.0, signature.1, signature.2),
                        (Some(pts), width, height),
                        "{name}, packet {index}"
                    );
                    if is_key {
                        expected.push(signature);
                    }
                    frame += 1;
                }
            }
            assert_eq!(
                frame,
                full_frames.len(),
                "{name}, packet {index}: extra output"
            );
            assert_eq!(
                key_frames.iter().map(signature).collect::<Vec<_>>(),
                expected,
                "{name}, packet {index}"
            );
            if let Some(last) = full_frames.last() {
                assert_eq!(
                    signature(last).3,
                    *published.get(shown).expect("no extra published output"),
                    "{name}, packet {index}"
                );
                shown += 1;
            }
            total += full_frames.len();
            keys += key_frames.len();
        }
        assert_eq!(shown, published.len(), "{name}: missing output");
        full.send_eof().unwrap();
        key.send_eof().unwrap();
        assert!(
            drain(full.as_mut()).is_empty(),
            "{name}: unexpected delayed output"
        );
        assert!(
            drain(key.as_mut()).is_empty(),
            "{name}: unexpected delayed key output"
        );
        vectors += 1;
    }
    assert_eq!(vectors, 341);
    assert!(total > 0 && keys > 0 && total > keys);
    println!("{vectors} vectors: {total} boundary frames, {keys} keyframe-only frames");
}
