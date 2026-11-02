//! One-IDR admission and requested-allocation bounds, including packed transfer.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use coreav_backend_h264::NativeH264;
use coreav_counting_alloc::{CountingAllocator, measure, report};
use coreav_decode::{Backend, Codec, ErrorKind, Limits, Packet, StreamConfig};
use coreav_frame::Pixels;
use coreav_h264::{Auto, Decoder, Error, Mode, OneIdrError, annex_b};

#[path = "../../coreav-h264/tests/common/streams.rs"]
mod streams;
use streams::*;

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn sps(a: u32, b: u32, mono: bool, crop_bottom: u32) -> Vec<u8> {
    sps_id(a, b, mono, crop_bottom, 0)
}

fn sps_id(a: u32, b: u32, mono: bool, crop_bottom: u32, id: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 100).u(8, 0).u(8, 51).ue(id);
    w.ue(u32::from(!mono)).ue(0).ue(0).flag(false).flag(false);
    w.ue(0)
        .ue(0)
        .ue(2)
        .ue(16)
        .flag(false)
        .ue(a - 1)
        .ue(b - 1)
        .flag(true)
        .flag(true)
        .flag(crop_bottom != 0);
    if crop_bottom != 0 {
        w.ue(0).ue(0).ue(0).ue(crop_bottom);
    }
    w.flag(false);
    nal(3, 7, w.finish())
}

fn slice(first: u32, count: usize, mono: bool) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(first)
        .ue(7)
        .ue(0)
        .u(4, 0)
        .ue(0)
        .u(6, 0)
        .flag(false)
        .flag(false)
        .se(0);
    for _ in 0..count {
        w.ue(3);
        if !mono {
            w.ue(0);
        }
        w.se(0).u(1, 1);
    }
    nal(3, 5, w.finish())
}

fn packet(a: u32, b: u32, mono: bool, split: bool, crop: u32) -> Vec<u8> {
    let mut data = sps(a, b, mono, crop);
    data.extend(nal(3, 8, pps()));
    let count = (a * b) as usize;
    if split {
        for first in 0..count {
            data.extend(slice(first as u32, 1, mono));
        }
    } else {
        data.extend(slice(0, count, mono));
    }
    data
}

fn decode(data: &[u8], limits: &Limits) -> Result<coreav_frame::Frame, coreav_decode::Error> {
    let mut decoder = NativeH264::one_idr().open(&StreamConfig::new(Codec::H264), limits)?;
    decoder.send_packet(&Packet {
        data,
        pts: Some(17),
        dts: Some(13),
        keyframe: true,
        discard: false,
    })?;
    decoder.send_eof()?;
    let frame = decoder.receive_frame()?.expect("one frame");
    assert!(decoder.receive_frame()?.is_none());
    Ok(frame)
}

#[test]
fn peak_allocations_fit_the_sps_bound_including_output_transfer() {
    let limits = Limits::default();
    let mut last = 1;
    while NativeH264::one_idr_memory_needed(
        annex_b(&sps(120, last + 1, false, 0)).next().unwrap(),
        &limits,
    )
    .is_ok()
    {
        last += 1;
    }
    let roster = [
        (1, 1, false, false, 0),
        (5, 1, false, true, 0),
        (9, 1, true, true, 0),
        (40, 23, false, false, 4),
        (80, 45, false, false, 0),
        (120, 68, false, false, 4),
        (68, 120, false, true, 0),
        (120, 68, true, true, 8),
        (120, last, false, true, 0),
    ];
    for (a, b, mono, split, crop) in roster {
        let data = packet(a, b, mono, split, crop);
        let bound = NativeH264::one_idr_memory_needed(annex_b(&data).next().unwrap(), &limits)
            .expect("admitted SPS");
        let (frame, counters) = measure(|| decode(&data, &limits).expect("complete IDR"));
        assert_eq!(frame.pts, Some(17));
        assert!(matches!(frame.pixels, Pixels::Cpu(_)));
        assert_eq!(counters.trough_bytes, 0);
        assert!(
            counters.peak_bytes <= bound,
            "{a}x{b}: {counters:?} > {bound}"
        );
        assert!(bound <= 32 << 20);
        report(
            &format!("one-idr-{a}x{b}-mono-{mono}-slices-{split}"),
            &counters,
        );
        println!(
            "one-IDR {a}x{b}: measured={} bound={bound}",
            counters.peak_bytes
        );
        let mut tight = limits.clone();
        tight.max_memory_bytes = bound - 1;
        let (result, refused) = measure(|| decode(&data, &tight));
        assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
        assert!(
            refused.peak_bytes < 128 << 10,
            "refusal allocated a picture: {refused:?}"
        );
    }
    let data = packet(120, last + 1, false, false, 0);
    let (result, refused) = measure(|| decode(&data, &limits));
    assert_eq!(result.unwrap_err().kind, ErrorKind::Limit);
    assert!(refused.peak_bytes < 128 << 10);
    report("one-idr-first-refused", &refused);
    println!("one-IDR first refused: 120x{} macroblocks", last + 1);

    let mut data = Vec::new();
    for id in 0..32 {
        data.extend(sps_id(120, 68, false, 4, id));
    }
    for id in 0..256 {
        let mut w = Writer::default();
        w.ue(id)
            .ue(0)
            .flag(false)
            .flag(false)
            .ue(0)
            .ue(0)
            .ue(0)
            .flag(false)
            .u(2, 0)
            .se(0)
            .se(0)
            .se(0)
            .flag(false)
            .flag(false)
            .flag(false);
        let mut raw = w.finish();
        raw.resize(4096, 0xff);
        data.extend(nal(3, 8, raw));
    }
    data.extend(slice(0, 8160, false));
    let bound = NativeH264::one_idr_memory_needed(annex_b(&data).next().unwrap(), &limits).unwrap();
    let (_, counters) = measure(|| decode(&data, &limits).expect("full parameter tables"));
    assert!(counters.peak_bytes <= bound);
    assert_eq!(counters.trough_bytes, 0);
    report("one-idr-full-parameter-tables", &counters);
    println!(
        "one-IDR full parameter tables: measured={} bound={bound}",
        counters.peak_bytes
    );
}

fn refused(data: &[u8]) -> Error {
    let mut decoder = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
    let error = decoder.decode_one_idr(data, None).expect_err("must refuse");
    assert!(decoder.take_pictures().is_empty());
    assert!(decoder.flush().is_err());
    assert!(decoder.take_pictures().is_empty());
    error
}

#[test]
fn admission_rejects_multiple_pictures_and_configuration_conflicts() {
    let good = packet(2, 2, false, false, 0);
    let mut two = good.clone();
    two.extend(slice(0, 4, false));
    assert_eq!(refused(&two), Error::OneIdr(OneIdrError::SecondPicture));
    let mut conflict = good.clone();
    conflict.extend(sps(3, 2, false, 0));
    assert_eq!(
        refused(&conflict),
        Error::OneIdr(OneIdrError::ConfigurationConflict)
    );
    let mut non_idr = sps(2, 2, false, 0);
    non_idr.extend(nal(3, 8, pps()));
    non_idr.extend(nal(3, 1, non_idr_pcm_slice_ref(0, 0, 4, true)));
    assert_eq!(refused(&non_idr), Error::OneIdr(OneIdrError::ExpectedIdr));
    let mut decoder = Decoder::<Auto>::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
    decoder.decode_one_idr(&good, None).expect("one IDR");
    assert_eq!(decoder.take_pictures().len(), 1);
    decoder.flush().expect("flush");
    assert_eq!(
        decoder.decode_one_idr(&good, None),
        Err(Error::OneIdr(OneIdrError::SecondPicture))
    );
    assert!(decoder.take_pictures().is_empty());
}

#[test]
fn incomplete_and_truncated_pictures_never_become_output() {
    let mut data = sps(2, 2, false, 0);
    data.extend(nal(3, 8, pps()));
    let mut missing = data.clone();
    missing.extend(slice(0, 3, false));
    assert_eq!(
        refused(&missing),
        Error::OneIdr(OneIdrError::IncompletePicture)
    );
    let mut gap = data.clone();
    gap.extend(slice(0, 1, false));
    gap.extend(slice(2, 2, false));
    assert_eq!(refused(&gap), Error::OneIdr(OneIdrError::IncompletePicture));
    let mut overlap = data.clone();
    overlap.extend(slice(0, 3, false));
    overlap.extend(slice(2, 2, false));
    assert!(matches!(refused(&overlap), Error::Invalid(_)));
    data.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
    data.truncate(data.len() - 100);
    assert!(matches!(refused(&data), Error::Truncated(_)));
}

#[test]
fn framing_and_streaming_cannot_bypass_admission() {
    let data = packet(2, 2, false, false, 0);
    for size in [1usize, 2, 4] {
        let mut framed = Vec::new();
        for unit in annex_b(&data) {
            let length = (unit.len() as u32).to_be_bytes();
            framed.extend_from_slice(&length[4 - size..]);
            framed.extend_from_slice(unit);
        }
        let mut decoder = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
        decoder
            .decode_one_idr(&framed, Some(size))
            .expect("length prefixes");
        assert_eq!(decoder.take_pictures().len(), 1);
        framed.push(0);
        let mut decoder = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
        assert!(decoder.decode_one_idr(&framed, Some(size)).is_err());
        assert!(decoder.take_pictures().is_empty());
    }
    let mut decoder = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
    for unit in annex_b(&data).take(2) {
        decoder.decode_nal(unit).expect("configuration");
    }
    assert_eq!(
        decoder.decode_nal(annex_b(&data).last().unwrap()),
        Err(Error::OneIdr(OneIdrError::AccessUnitRequired))
    );
    assert!(decoder.take_pictures().is_empty());
    let mut delimited = data.clone();
    delimited.extend(nal(0, 9, vec![0x10]));
    delimited.extend(slice(4, 1, false));
    assert_eq!(
        refused(&delimited),
        Error::OneIdr(OneIdrError::SecondPicture)
    );
    for tail in [&[0, 0, 1][..], &[0, 0, 0, 1], &[0, 0, 1, 0, 0, 1, 0x65]] {
        let mut broken = data.clone();
        broken.extend_from_slice(tail);
        assert!(matches!(refused(&broken), Error::Truncated(_)));
    }
    let mut trailing = data.clone();
    trailing.extend_from_slice(&[0, 0, 0, 0]);
    assert!(decode(&trailing, &Limits::default()).is_ok());
}

#[test]
fn failed_submissions_discard_undrained_output() {
    let data = packet(2, 2, false, false, 0);
    let mut codec = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
    codec.decode_one_idr(&data, None).unwrap();
    assert_eq!(
        codec.decode_one_idr(&data, None),
        Err(Error::OneIdr(OneIdrError::SecondPicture))
    );
    assert!(codec.take_pictures().is_empty());
    assert!(codec.flush().is_err());
    let mut codec = Decoder::new(coreav_h264::Limits::default()).with_mode(Mode::OneIdr);
    codec.decode_one_idr(&data, None).unwrap();
    assert!(codec.decode_nal(annex_b(&data).next().unwrap()).is_err());
    assert!(codec.take_pictures().is_empty());
    for limited in [false, true] {
        let limits = Limits {
            max_packet_bytes: data.len() as u64,
            ..Default::default()
        };
        let mut backend = NativeH264::one_idr()
            .open(&StreamConfig::new(Codec::H264), &limits)
            .unwrap();
        let mut packet = Packet {
            data: &data,
            pts: None,
            dts: None,
            keyframe: true,
            discard: false,
        };
        backend.send_packet(&packet).unwrap();
        let too_large = vec![0; data.len() + 1];
        if limited {
            packet.data = &too_large;
        }
        assert!(backend.send_packet(&packet).is_err());
        assert!(backend.receive_frame().unwrap().is_none());
        assert!(backend.send_eof().is_err());
    }
}

#[test]
fn a_streaming_decoder_cannot_import_large_scratch_into_one_idr() {
    let mut codec = Decoder::new(coreav_h264::Limits::default());
    let mut malformed = vec![0xff; (32 << 20) + 1];
    malformed[0] = 0x65;
    assert!(codec.decode_nal(&malformed).is_err());
    let mut codec = codec.with_mode(Mode::OneIdr);
    assert_eq!(
        codec.decode_one_idr(&packet(2, 2, false, false, 0), None),
        Err(Error::OneIdr(OneIdrError::Failed))
    );
    assert!(codec.take_pictures().is_empty());
}

#[test]
fn gap_frame_numbers_and_predicted_idr_slices_are_typed_errors() {
    for (predicted, frame_num, want) in [
        (false, 2, OneIdrError::FrameNumGap),
        (true, 0, OneIdrError::PredictedSlice),
    ] {
        let mut w = Writer::default();
        w.ue(0)
            .ue(if predicted { 5 } else { 7 })
            .ue(0)
            .u(4, frame_num)
            .ue(0)
            .u(6, 0);
        if predicted {
            w.flag(false).flag(false);
        }
        w.flag(false).flag(false).se(0);
        let mut data = nal(3, 7, sps_with_gaps(2, 2));
        data.extend(nal(3, 8, pps()));
        data.extend(nal(3, 5, w.finish()));
        assert_eq!(refused(&data), Error::OneIdr(want));
    }
}

#[test]
#[ignore = "needs the pinned ffmpeg binary on Linux CI"]
fn generated_entropy_modes_and_pcm_fit_the_same_bound() {
    for cabac in [0, 1] {
        for (width, height) in [(640, 360), (1280, 720), (1920, 1080), (1080, 1920)] {
            let input = format!("testsrc2=size={width}x{height}:rate=1");
            let params = format!("cabac={cabac}:slices=4:keyint=1");
            let output = std::process::Command::new("ffmpeg")
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &input,
                    "-frames:v",
                    "1",
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-x264-params",
                    &params,
                    "-f",
                    "h264",
                    "-",
                ])
                .output()
                .expect("pinned ffmpeg");
            assert!(output.status.success());
            let data = output.stdout;
            let limits = Limits::default();
            let sps = annex_b(&data).find(|unit| unit[0] & 31 == 7).unwrap();
            let bound = NativeH264::one_idr_memory_needed(sps, &limits).unwrap();
            let (frame, counters) = measure(|| decode(&data, &limits).expect("generated IDR"));
            assert!(counters.peak_bytes <= bound);
            assert_eq!(counters.trough_bytes, 0);
            let mut normal = NativeH264::with_options(coreav_backend_h264::NativeOptions {
                threads: Some(1),
                ..Default::default()
            })
            .open(&StreamConfig::new(Codec::H264), &limits)
            .unwrap();
            normal
                .send_packet(&Packet {
                    data: &data,
                    pts: Some(17),
                    dts: Some(13),
                    keyframe: true,
                    discard: false,
                })
                .unwrap();
            normal.send_eof().unwrap();
            let reference = normal.receive_frame().unwrap().unwrap();
            assert_eq!(frame.pts, reference.pts);
            let mut ours = Vec::new();
            let mut theirs = Vec::new();
            frame.to_cpu().unwrap().write_packed(&mut ours).unwrap();
            reference
                .to_cpu()
                .unwrap()
                .write_packed(&mut theirs)
                .unwrap();
            assert_eq!(ours, theirs);
            report(
                &format!("one-idr-generated-{width}x{height}-cabac-{cabac}"),
                &counters,
            );
            println!(
                "one-IDR generated {width}x{height} cabac={cabac}: measured={} bound={bound}",
                counters.peak_bytes
            );
        }
    }
    let mut data = sps(120, 68, false, 4);
    data.extend(nal(3, 8, pps()));
    data.extend(nal(3, 5, idr_pcm_slice(8160, |_| [128; 384])));
    let bound =
        NativeH264::one_idr_memory_needed(annex_b(&data).next().unwrap(), &Limits::default())
            .unwrap();
    let (_, counters) = measure(|| decode(&data, &Limits::default()).expect("large PCM IDR"));
    assert!(counters.peak_bytes <= bound);
    report("one-idr-pcm-1920x1088", &counters);
    println!(
        "one-IDR PCM: measured={} bound={bound}",
        counters.peak_bytes
    );
}
