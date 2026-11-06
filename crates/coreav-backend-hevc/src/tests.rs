use super::*;
use coreav_decode::{CodecConfig, ConfigKind, Policy};
use coreav_frame::{ColorInfo, Pixels};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[path = "../../coreav-hevc/test-support/pcm.rs"]
mod pcm;
#[path = "../../coreav-hevc/test-support/writer.rs"]
mod writer;

fn access_unit(nals: impl IntoIterator<Item = impl AsRef<[u8]>>, length: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for nal in nals {
        let nal = nal.as_ref();
        if length {
            bytes.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        } else {
            bytes.extend_from_slice(&[0, 0, 0, 1]);
        }
        bytes.extend_from_slice(nal);
    }
    bytes
}

fn configuration(sets: &[Vec<u8>; 3], depth: u32) -> CodecConfig {
    let mut data = vec![1, if depth == 10 { 2 } else { 1 }];
    data.resize(16, 0);
    data.extend_from_slice(&[1, (depth - 8) as u8, (depth - 8) as u8, 0, 0]);
    data.extend_from_slice(&[3, 3]);
    for (kind, nal) in (32..=34).zip(sets) {
        data.push(kind);
        data.extend_from_slice(&[0, 1]);
        data.extend_from_slice(&(nal.len() as u16).to_be_bytes());
        data.extend_from_slice(nal);
    }
    CodecConfig {
        kind: ConfigKind::Hvcc,
        data,
    }
}

fn packet(data: &[u8], pts: Option<i64>, discard: bool) -> Packet<'_> {
    Packet {
        data,
        pts,
        dts: pts,
        keyframe: true,
        discard,
    }
}

fn terminal(decoder: &mut dyn Decoder) {
    assert!(decoder.receive_frame().is_err());
    assert!(decoder.send_eof().is_err());
    assert!(decoder.send_packet(&packet(&[], None, false)).is_err());
    assert_eq!(decoder.info().backend, BackendKind::Native);
    assert_eq!(decoder.info().isolation, Isolation::InProcess);
}

#[test]
fn native_frames_preserve_depth_pts_and_discard_through_eof() -> Result<()> {
    for depth in [8, 10] {
        let fixture = pcm::pcm_fixture(depth, depth, false)?;
        for length in [false, true] {
            for pts in [None, Some(-9001)] {
                let mut stream = StreamConfig::new(Codec::Hevc);
                if length {
                    stream.config = Some(configuration(&fixture.sets, depth));
                }
                let mut decoder = NativeHevc::new().open(&stream, &Limits::default())?;
                let first = if length {
                    access_unit([&fixture.nal], true)
                } else {
                    access_unit(fixture.sets.iter().chain([&fixture.nal]), false)
                };
                let next = access_unit([&fixture.nal], length);
                let mut frames = Vec::new();
                for (bytes, discard) in [(&first, false), (&next, true), (&next, false)] {
                    decoder.send_packet(&packet(bytes, pts, discard))?;
                    while let Some(frame) = decoder.receive_frame()? {
                        frames.push(frame);
                    }
                }
                decoder.send_eof()?;
                decoder.send_eof()?;
                while let Some(frame) = decoder.receive_frame()? {
                    frames.push(frame);
                }
                assert!(decoder.receive_frame()?.is_none());
                assert_eq!(frames.len(), 2);
                for frame in frames {
                    assert_eq!(frame.pts, pts);
                    let Pixels::Cpu(picture) = frame.pixels else {
                        return Err("not CPU output".into());
                    };
                    assert_eq!(
                        (picture.width(), picture.height(), picture.format().depth()),
                        (8, 8, depth as u8)
                    );
                    for (plane, samples) in picture.planes().iter().zip(&fixture.planes) {
                        let expected: Vec<u8> = samples
                            .iter()
                            .flat_map(|value| {
                                value
                                    .to_le_bytes()
                                    .into_iter()
                                    .take(if depth == 8 { 1 } else { 2 })
                            })
                            .collect();
                        assert_eq!(plane.data(), expected);
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn malformed_or_multi_picture_access_units_close_and_discard_queued_output() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    for bad in [
        access_unit([&fixture.nal, &fixture.nal], false),
        access_unit([fixture.nal.as_slice(), &[0, 0, 0x80]], false),
    ] {
        let mut decoder =
            NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &Limits::default())?;
        decoder.send_packet(&packet(&access_unit(&fixture.sets, false), None, false))?;
        assert!(decoder.send_packet(&packet(&bad, Some(7), false)).is_err());
        terminal(&mut *decoder);
    }
    Ok(())
}

#[test]
fn a_packet_after_eof_fails_and_drops_unreceived_frames() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let data = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    let mut decoder =
        NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &Limits::default())?;
    decoder.send_packet(&packet(&data, Some(17), false))?;
    decoder.send_eof()?;
    assert!(
        decoder
            .send_packet(&packet(&data, Some(18), false))
            .is_err()
    );
    terminal(&mut *decoder);
    Ok(())
}

#[test]
fn lowered_resource_limits_cannot_be_bypassed_by_direct_backend_open() -> Result<()> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    let data = access_unit(fixture.sets.iter().chain([&fixture.nal]), false);
    for limits in [
        Limits {
            max_pixels: 63,
            ..Limits::default()
        },
        Limits {
            max_reference_frames: 0,
            ..Limits::default()
        },
        Limits {
            max_frame_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_decoded_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_pending_bytes: 95,
            ..Limits::default()
        },
        Limits {
            max_packet_bytes: data.len() as u64 - 1,
            ..Limits::default()
        },
        Limits {
            max_memory_bytes: 0,
            ..Limits::default()
        },
    ] {
        let opened = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &limits);
        let mut decoder = match opened {
            Ok(decoder) => decoder,
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Limit);
                continue;
            }
        };
        let result = decoder
            .send_packet(&packet(&data, None, false))
            .and_then(|()| decoder.send_eof());
        let error = result.err().ok_or("lowered limit did not fail")?;
        assert_eq!(error.kind, ErrorKind::Limit);
        terminal(&mut *decoder);
    }
    Ok(())
}

#[test]
fn zero_session_deadline_is_typed_timeout() {
    let limits = Limits {
        timeout: std::time::Duration::ZERO,
        ..Limits::default()
    };
    let opened = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), &limits);
    assert!(matches!(
        opened,
        Err(Error {
            kind: ErrorKind::Timeout,
            ..
        })
    ));
    assert_eq!(
        codec_error(coreav_hevc::Error::Limit("decode_deadline")).kind,
        ErrorKind::Timeout
    );
}

#[test]
fn crop_and_colour_apply_once_and_keep_absent_fields() -> Result<()> {
    let fixture = pcm::pcm_fixture(10, 10, false)?;
    let mut decoder = PictureDecoder::new(DecodeLimits::default());
    let container = ColorInfo {
        primaries: Some(1),
        transfer: Some(16),
        matrix: Some(1),
        full_range: Some(true),
        ..ColorInfo::default()
    };
    let mut output = Output::new(
        Limits::default(),
        Limits::default().deadline(),
        container,
        0,
    )?;
    decoder.set_external_memory(output.retained_bytes()?)?;
    for set in &fixture.sets {
        decoder.read_nal(set, None, |_| Ok(()))?;
    }
    decoder.read_nal_with_origin(&fixture.nal, Some(99), 12, |_, _| Ok(()))?;
    decoder.finish_with_output(|mut picture, memory| {
        picture.info.visible_width = 4;
        picture.info.visible_height = 4;
        picture.info.conformance_window = [1; 4];
        picture.info.vui.colour_primaries = Some(9);
        picture.info.vui.full_range = Some(false);
        picture.info.vui.chroma_location = Some((2, 2));
        output
            .push(picture, memory)
            .map_err(|_| coreav_hevc::Error::InvalidData("test_output"))
    })?;
    let frame = output.pop().ok_or("missing cropped frame")?;
    assert_eq!(frame.pts, Some(99));
    assert_eq!(
        frame.color,
        ColorInfo {
            primaries: Some(9),
            transfer: Some(16),
            matrix: Some(1),
            full_range: Some(false),
            chroma_location: Some(coreav_frame::ChromaLocation::TopLeft)
        }
    );
    let Pixels::Cpu(picture) = frame.pixels else {
        return Err("not CPU output".into());
    };
    assert_eq!((picture.width(), picture.height()), (4, 4));
    for (component, (plane, original)) in picture.planes().iter().zip(&fixture.planes).enumerate() {
        let (side, offset, visible) = if component == 0 { (8, 2, 4) } else { (4, 1, 2) };
        let mut expected = Vec::new();
        for row in offset..offset + visible {
            let values = original
                .get(row * side + offset..row * side + offset + visible)
                .ok_or("short original plane")?;
            expected.extend(values.iter().flat_map(|value| value.to_le_bytes()));
        }
        assert_eq!(plane.data(), expected);
    }
    Ok(())
}

#[test]
fn invalid_configuration_and_other_codecs_are_never_admitted() {
    assert!(matches!(
        NativeHevc::new().supports(&StreamConfig::new(Codec::H264), &StreamTraits::default()),
        Support::No(_)
    ));
    for data in [vec![], vec![1; 22], vec![0; 23]] {
        let mut stream = StreamConfig::new(Codec::Hevc);
        stream.config = Some(CodecConfig {
            kind: ConfigKind::Hvcc,
            data,
        });
        assert!(NativeHevc::new().open(&stream, &Limits::default()).is_err());
    }
    let admitted = |tail: &[u8], length_size: u8| {
        let mut data = vec![0; 22];
        if let Some(first) = data.first_mut() {
            *first = 1;
        }
        if let Some(size) = data.get_mut(21) {
            *size = 0xfc | length_size;
        }
        data.extend_from_slice(tail);
        let mut stream = StreamConfig::new(Codec::Hevc);
        stream.config = Some(CodecConfig {
            kind: ConfigKind::Hvcc,
            data,
        });
        matches!(
            NativeHevc::new().supports(&stream, &StreamTraits::default()),
            Support::Yes
        )
    };
    assert!(admitted(&[0], 3));
    assert!(admitted(&[1, 0x20, 0, 1, 0, 2, 0x40, 1], 3));
    assert!(admitted(
        &[2, 0x20, 0, 1, 0, 2, 0x40, 1, 0x21, 0, 1, 0, 2, 0x42, 1],
        0
    ));
    assert!(
        !admitted(&[0], 2),
        "length size three is not an hvcC encoding"
    );
    assert!(!admitted(&[65], 3), "more than 64 arrays");
    assert!(!admitted(&[0, 0], 3), "trailing bytes");
    assert!(
        !admitted(&[2, 0x20, 0, 0, 0x20, 0, 0], 3),
        "duplicate array"
    );
    assert!(!admitted(&[1, 0x60, 0, 0], 3), "reserved array bit");
    assert!(
        !admitted(&[1, 0x21, 0, 1, 0, 2, 0x40, 1], 3),
        "array type mismatch"
    );
    assert!(
        !admitted(&[1, 0x01, 0, 1, 0, 2, 0x02, 1], 3),
        "slice data in hvcC"
    );
    assert!(
        !admitted(&[1, 0x20, 0, 2, 0, 2, 0x40, 1], 3),
        "truncated array"
    );
    let mut many = vec![1, 0x20, 0x10, 0x01];
    for _ in 0..4097 {
        many.extend_from_slice(&[0, 2, 0x40, 1]);
    }
    assert!(!admitted(&many, 3), "more than 4096 configuration NALs");
}

#[test]
fn selected_native_errors_never_open_a_fallback() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Spy(AtomicUsize);
    impl Backend for Spy {
        fn kind(&self) -> BackendKind {
            BackendKind::Fallback
        }
        fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
            Support::Yes
        }
        fn open(
            &self,
            _: &StreamConfig,
            _: &Limits,
        ) -> std::result::Result<Box<dyn Decoder>, Error> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(invalid("fallback spy was opened"))
        }
    }
    let spy = Spy(AtomicUsize::new(0));
    let native = NativeHevc::new();
    let mut opened = coreav_decode::open(
        &StreamConfig::new(Codec::Hevc),
        &Policy::default(),
        &Limits::default(),
        &[&native, &spy],
    )?;
    assert!(
        opened
            .decoder
            .send_packet(&packet(&[0, 0, 1, 0, 0], None, false))
            .is_err()
    );
    terminal(&mut *opened.decoder);
    assert_eq!(spy.0.load(Ordering::Relaxed), 0);
    Ok(())
}
