//! Hostile, incremental, and terminal worker-transport regressions.

use coreav_backend_svt_av1::{Error, model, protocol};

use std::io::{self, Read, Write};
use std::{cell::Cell, rc::Rc};

use model::{Color, Retention, Settings};
use protocol::{Decoder, InputReader, Limits, OutputWriter, Record, Request};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request() -> Result<(Request, Retention), Error> {
    let settings = Settings::new(64, 64, (30_000, 1001), 8, 32, Color::new(1, 1, 1, true, 2)?)?;
    let budget = 32 + 2 * (settings.raw_bytes() as u64 + 32);
    let limits = Limits::new(budget, 1 << 20, 16, 1024)?;
    let retention = Retention::new(settings, budget, 16, 16, 16)?;
    Ok((Request { settings, limits }, retention))
}

fn tlv(wire: &mut Vec<u8>, tag: u8, payload: &[u8]) {
    wire.push(tag);
    wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wire.extend_from_slice(payload);
}

fn begin(base: u64, retention: Retention, keyint: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in [
        base,
        retention.budget(),
        retention.fixed_bytes(),
        retention.picture_bytes(),
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&retention.pictures().to_le_bytes());
    bytes.extend_from_slice(&keyint.to_le_bytes());
    bytes
}

fn packet(flags: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(-73i64).to_le_bytes());
    bytes.extend_from_slice(&(-81i64).to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&99u32.to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

fn end(base: u64, submitted: u32, packets: u64, peak: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&base.to_le_bytes());
    bytes.extend_from_slice(&submitted.to_le_bytes());
    bytes.extend_from_slice(&packets.to_le_bytes());
    bytes.extend_from_slice(&peak.to_le_bytes());
    bytes
}

fn start() -> Vec<u8> {
    let mut wire = protocol::OUTPUT_MAGIC.to_vec();
    tlv(&mut wire, 1, b"4.2.0");
    wire
}

fn segment(
    wire: &mut Vec<u8>,
    request: Request,
    retention: Retention,
    base: u64,
    count: u32,
) -> Result<(), Error> {
    tlv(wire, 2, &begin(base, retention, request.settings.keyint()));
    tlv(wire, 3, &packet(0, &[17, 25, 38]));
    tlv(wire, 3, &packet(1, &[]));
    tlv(wire, 4, &end(base, count, 2, retention.peak_bytes(count)?));
    Ok(())
}

fn complete(wire: &mut Vec<u8>, frames: u64, segments: u64) {
    let mut bytes = frames.to_le_bytes().to_vec();
    bytes.extend_from_slice(&segments.to_le_bytes());
    tlv(wire, 5, &bytes);
}

fn valid_wire(request: Request, retention: Retention) -> Result<Vec<u8>, Error> {
    let mut wire = start();
    segment(&mut wire, request, retention, 0, 2)?;
    segment(&mut wire, request, retention, 2, 1)?;
    complete(&mut wire, 3, 2);
    Ok(wire)
}

fn decode(request: Request, retention: Retention, wire: &[u8]) -> Result<Vec<Record>, Error> {
    let mut decoder = Decoder::new(request, retention)?;
    let mut records = Vec::new();
    decoder.push(wire, |record| {
        records.push(record);
        Ok(())
    })?;
    decoder.finish()?;
    Ok(records)
}

#[test]
fn input_header_is_fixed_little_endian_and_preserves_settings() -> TestResult {
    let (request, _) = request()?;
    let mut wire = Vec::new();
    request.write(&mut wire)?;
    assert_eq!(wire.len(), 60);
    assert_eq!(wire.get(..8), Some(protocol::INPUT_MAGIC.as_slice()));
    assert_eq!(wire.get(8..12), Some(64u32.to_le_bytes().as_slice()));
    assert_eq!(wire.get(16..20), Some(30_000u32.to_le_bytes().as_slice()));
    assert_eq!(wire.get(20..24), Some(1001u32.to_le_bytes().as_slice()));
    assert_eq!(wire.get(24..32), Some([8, 32, 1, 1, 1, 1, 2, 0].as_slice()));
    assert_eq!(Request::read(&mut wire.as_slice())?, request);
    assert_eq!(
        request.limits.retained_bytes(),
        32 + 2 * (request.settings.raw_bytes() as u64 + 32)
    );
    assert_eq!(request.limits.output_bytes(), 1 << 20);
    assert_eq!(request.limits.frames(), 16);
    assert_eq!(request.limits.packet_bytes(), 1024);
    for size in 0..wire.len() {
        let mut truncated = wire.get(..size).ok_or("extent")?;
        assert!(matches!(
            Request::read(&mut truncated),
            Err(Error::Output(_))
        ));
    }
    Ok(())
}

#[test]
fn header_flags_settings_and_every_limit_are_checked() -> TestResult {
    let (request, _) = request()?;
    let mut wire = Vec::new();
    request.write(&mut wire)?;
    for offset in [0, 8, 12, 24, 25, 26, 27, 28, 29, 30, 31] {
        let mut changed = wire.clone();
        *changed.get_mut(offset).ok_or("extent")? = 255;
        assert!(
            Request::read(&mut changed.as_slice()).is_err(),
            "offset {offset}"
        );
    }
    for (offset, length) in [(16, 4), (20, 4), (32, 8), (40, 8), (48, 8), (56, 4)] {
        let mut changed = wire.clone();
        changed
            .get_mut(offset..offset + length)
            .ok_or("extent")?
            .fill(0);
        assert!(Request::read(&mut changed.as_slice()).is_err());
    }
    assert!(Limits::new(model::RETAINED_BYTES + 1, 100, 1, 1).is_err());
    assert!(Limits::new(1, protocol::MAX_OUTPUT_BYTES + 1, 1, 1).is_err());
    assert!(Limits::new(1, 100, 1, model::MAX_PACKET_BYTES as u32 + 1).is_err());
    assert!(
        Limits::new(
            model::RETAINED_BYTES,
            protocol::MAX_OUTPUT_BYTES,
            u64::MAX,
            model::MAX_PACKET_BYTES as u32
        )
        .is_ok()
    );
    Ok(())
}

struct CountedReader {
    bytes: std::io::Cursor<Vec<u8>>,
    count: Rc<Cell<usize>>,
}

impl Read for CountedReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.read(bytes)?;
        self.count.set(self.count.get() + count);
        Ok(count)
    }
}

#[test]
fn prefixes_do_not_read_pixels_and_owned_pictures_have_exact_capacity() -> TestResult {
    let (request, _) = request()?;
    let mut wire = Vec::new();
    request.write(&mut wire)?;
    for (id, pts) in [(0, -50), (1, 100), (2, 127)] {
        protocol::write_frame_header(&mut wire, id, pts)?;
        wire.extend(std::iter::repeat_n(id as u8, request.settings.raw_bytes()));
    }
    protocol::write_input_end(&mut wire, 3)?;
    let count = Rc::new(Cell::new(0));
    let mut reader = InputReader::new(CountedReader {
        bytes: io::Cursor::new(wire),
        count: count.clone(),
    })?;
    assert_eq!(reader.request(), request);
    let mut pictures = Vec::new();
    for (id, pts) in [(0, -50), (1, 100), (2, 127)] {
        let before = count.get();
        assert_eq!(
            reader.next_frame_header()?,
            Some(protocol::FrameHeader { id, pts })
        );
        assert_eq!(count.get() - before, 17);
        let picture = reader.read_picture()?;
        assert_eq!(picture.capacity(), request.settings.raw_bytes());
        assert_eq!(picture.len(), request.settings.raw_bytes());
        assert!(picture.iter().all(|value| *value == id as u8));
        pictures.push(picture);
    }
    assert!(reader.next_frame_header()?.is_none());
    assert!(reader.next_frame_header()?.is_none());
    assert_eq!(pictures.len(), 3);
    Ok(())
}

#[test]
fn input_rejects_identity_pts_truncation_order_and_false_completion() -> TestResult {
    let (request, _) = request()?;
    let mut header = Vec::new();
    request.write(&mut header)?;
    for (id, pts, expected) in [(1, 5, false), (0, 5, true)] {
        let mut wire = header.clone();
        protocol::write_frame_header(&mut wire, id, pts)?;
        let mut input = InputReader::new(wire.as_slice())?;
        assert_eq!(input.next_frame_header().is_ok(), expected);
        assert!(input.next_frame_header().is_err());
        assert!(input.read_picture().is_err());
    }
    for pts in [5, 4, i64::MIN] {
        let mut wire = header.clone();
        protocol::write_frame_header(&mut wire, 0, 5)?;
        wire.resize(wire.len() + request.settings.raw_bytes(), 0);
        protocol::write_frame_header(&mut wire, 1, pts)?;
        let mut input = InputReader::new(wire.as_slice())?;
        input.next_frame_header()?;
        input.read_picture()?;
        assert!(input.next_frame_header().is_err());
    }
    for tail in [vec![], vec![1], vec![3], vec![2, 0], {
        let mut bytes = vec![2];
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes
    }] {
        let mut wire = header.clone();
        wire.extend_from_slice(&tail);
        let mut input = InputReader::new(wire.as_slice())?;
        assert!(matches!(input.next_frame_header(), Err(Error::Output(_))));
        assert!(input.next_frame_header().is_err());
    }
    let mut input = InputReader::new(header.as_slice())?;
    assert!(input.read_picture().is_err());
    assert!(input.next_frame_header().is_err());
    for final_count in [0, 2] {
        let mut wire = header.clone();
        protocol::write_frame_header(&mut wire, 0, 5)?;
        wire.resize(wire.len() + request.settings.raw_bytes(), 0);
        protocol::write_input_end(&mut wire, final_count)?;
        let mut input = InputReader::new(wire.as_slice())?;
        input.next_frame_header()?;
        input.read_picture()?;
        assert!(input.next_frame_header().is_err());
    }
    let mut wire = header;
    protocol::write_frame_header(&mut wire, 0, i64::MAX)?;
    wire.resize(wire.len() + request.settings.raw_bytes(), 0);
    protocol::write_input_end(&mut wire, 1)?;
    wire.push(0);
    let mut input = InputReader::new(wire.as_slice())?;
    input.next_frame_header()?;
    input.read_picture()?;
    assert!(input.next_frame_header().is_err());
    Ok(())
}

#[test]
fn input_frame_limit_and_pixel_truncation_are_terminal() -> TestResult {
    let (mut request, _) = request()?;
    request.limits = Limits::new(request.limits.retained_bytes(), 1000, 1, 100)?;
    let mut wire = Vec::new();
    request.write(&mut wire)?;
    protocol::write_frame_header(&mut wire, 0, 0)?;
    wire.resize(wire.len() + request.settings.raw_bytes(), 0);
    protocol::write_frame_header(&mut wire, 1, 1)?;
    let mut input = InputReader::new(wire.as_slice())?;
    input.next_frame_header()?;
    input.read_picture()?;
    assert!(input.next_frame_header().is_err());
    wire.truncate(60 + 17 + request.settings.raw_bytes() - 1);
    let mut input = InputReader::new(wire.as_slice())?;
    input.next_frame_header()?;
    assert!(matches!(input.read_picture(), Err(Error::Output(_))));
    assert!(input.read_picture().is_err());
    Ok(())
}

#[test]
fn output_is_identical_for_every_two_chunk_split_and_single_bytes() -> TestResult {
    let (request, retention) = request()?;
    let wire = valid_wire(request, retention)?;
    let expected = decode(request, retention, &wire)?;
    assert_eq!(
        expected.last(),
        Some(&Record::Complete {
            frames: 3,
            segments: 2
        })
    );
    assert!(expected.iter().any(|record| matches!(record, Record::Packet { pts: -73, dts: -81, picture_type: 99, data, .. } if data == &[17,25,38])));
    for split in 0..=wire.len() {
        let mut decoder = Decoder::new(request, retention)?;
        let mut records = Vec::new();
        let (first, second) = wire.split_at(split);
        for chunk in [first, second] {
            decoder.push(chunk, |record| {
                records.push(record);
                Ok(())
            })?;
        }
        decoder.finish()?;
        assert_eq!(records, expected, "split {split}");
    }
    let mut decoder = Decoder::new(request, retention)?;
    let mut records = Vec::new();
    for chunk in wire.chunks(1) {
        decoder.push(chunk, |record| {
            records.push(record);
            Ok(())
        })?;
    }
    decoder.finish()?;
    assert_eq!(records, expected);
    Ok(())
}

#[test]
fn every_output_truncation_and_trailing_byte_is_rejected() -> TestResult {
    let (request, retention) = request()?;
    let wire = valid_wire(request, retention)?;
    for size in 0..wire.len() {
        let mut decoder = Decoder::new(request, retention)?;
        assert!(
            decoder
                .push(wire.get(..size).ok_or("extent")?, |_| Ok(()))
                .is_ok()
        );
        assert!(decoder.finish().is_err(), "truncation {size}");
        assert!(decoder.push(&wire, |_| Ok(())).is_err());
    }
    let mut decoder = Decoder::new(request, retention)?;
    decoder.push(&wire, |_| Ok(()))?;
    decoder.finish()?;
    assert!(decoder.push(&[0], |_| Ok(())).is_err());
    assert!(decoder.finish().is_err());
    let mut duplicate = wire.clone();
    complete(&mut duplicate, 3, 2);
    assert!(decode(request, retention, &duplicate).is_err());
    Ok(())
}

#[test]
fn control_order_lengths_and_version_are_closed() -> TestResult {
    let (request, retention) = request()?;
    for tag in 0..=255 {
        let mut wire = protocol::OUTPUT_MAGIC.to_vec();
        wire.push(tag);
        wire.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut decoder = Decoder::new(request, retention)?;
        assert!(decoder.push(&wire, |_| Ok(())).is_err(), "tag {tag}");
    }
    for version in [b"v4.2.0".as_slice(), b"4.2.0".as_slice()] {
        let mut wire = protocol::OUTPUT_MAGIC.to_vec();
        tlv(&mut wire, 1, version);
        segment(&mut wire, request, retention, 0, 1)?;
        complete(&mut wire, 1, 1);
        assert!(decode(request, retention, &wire).is_ok());
    }
    for version in [
        b"4.2.1".as_slice(),
        b"4.2.0\0".as_slice(),
        b"V4.2.0".as_slice(),
        &[255; 5],
    ] {
        let mut wire = protocol::OUTPUT_MAGIC.to_vec();
        tlv(&mut wire, 1, version);
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    for (tag, payload) in [
        (1, b"4.2.0".to_vec()),
        (3, packet(0, &[1])),
        (4, end(0, 1, 0, retention.bytes())),
        (5, vec![0; 16]),
    ] {
        let mut wire = start();
        tlv(&mut wire, tag, &payload);
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    for (tag, length) in [(2, 39), (2, 41), (3, 23), (3, 1049)] {
        let mut wire = start();
        if tag == 3 {
            tlv(
                &mut wire,
                2,
                &begin(0, retention, request.settings.keyint()),
            );
        }
        wire.push(tag);
        wire.extend_from_slice(&(length as u32).to_le_bytes());
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn every_segment_envelope_scalar_is_bound_and_short_tail_is_final() -> TestResult {
    let (request, retention) = request()?;
    let expected = begin(0, retention, request.settings.keyint());
    for offset in [0, 8, 16, 24, 32, 36] {
        let mut changed = expected.clone();
        *changed.get_mut(offset).ok_or("extent")? ^= 1;
        let mut wire = start();
        tlv(&mut wire, 2, &changed);
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    for (count, base) in [(1, 1), (2, 0), (2, 1), (2, 3)] {
        let mut wire = start();
        segment(&mut wire, request, retention, 0, count)?;
        tlv(
            &mut wire,
            2,
            &begin(base, retention, request.settings.keyint()),
        );
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    let mut wire = start();
    tlv(&mut wire, 2, &expected);
    tlv(&mut wire, 2, &expected);
    assert!(
        Decoder::new(request, retention)?
            .push(&wire, |_| Ok(()))
            .is_err()
    );
    let different = Retention::new(request.settings, retention.budget() - 1, 16, 16, 16)?;
    assert!(Decoder::new(request, different).is_err());
    Ok(())
}

#[test]
fn eos_failure_flags_and_native_packet_count_are_enforced() -> TestResult {
    let (request, retention) = request()?;
    for (flags, data) in [
        (0, vec![]),
        (2, vec![]),
        (3, vec![]),
        (16, vec![1]),
        (1 << 31, vec![1]),
    ] {
        let mut wire = start();
        tlv(
            &mut wire,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        tlv(&mut wire, 3, &packet(flags, &data));
        let error = Decoder::new(request, retention)?
            .push(&wire, |_| Ok(()))
            .err()
            .ok_or("expected error")?;
        if flags & !15 != 0 {
            assert!(matches!(error, Error::Native(_)));
        }
    }
    for flags in 0..16 {
        let mut wire = start();
        tlv(
            &mut wire,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        tlv(&mut wire, 3, &packet(flags, &[42]));
        if flags & 1 == 0 {
            tlv(&mut wire, 3, &packet(1, &[]));
        }
        let packets = if flags & 1 == 0 { 2 } else { 1 };
        tlv(&mut wire, 4, &end(0, 1, packets, retention.peak_bytes(1)?));
        complete(&mut wire, 1, 1);
        assert!(decode(request, retention, &wire).is_ok());
    }
    for payload in [packet(1, &[]), packet(0, &[1])] {
        let mut wire = start();
        tlv(
            &mut wire,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        tlv(&mut wire, 3, &packet(1, &[]));
        tlv(&mut wire, 3, &payload);
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    let mut wire = start();
    tlv(
        &mut wire,
        2,
        &begin(0, retention, request.settings.keyint()),
    );
    for _ in 0..4 * retention.pictures() + 32 {
        tlv(&mut wire, 3, &packet(0, &[1]));
    }
    let mut decoder = Decoder::new(request, retention)?;
    decoder.push(&wire, |_| Ok(()))?;
    let mut extra = Vec::new();
    tlv(&mut extra, 3, &packet(1, &[]));
    assert!(decoder.push(&extra, |_| Ok(())).is_err());
    Ok(())
}

#[test]
fn short_and_full_segments_require_the_exact_peak_including_reserved_slots() -> TestResult {
    let (request, retention) = request()?;
    for (submitted, expected) in [(1, 6224), (2, 12384)] {
        for peak in [expected - 1, expected, expected + 1] {
            let mut wire = start();
            tlv(
                &mut wire,
                2,
                &begin(0, retention, request.settings.keyint()),
            );
            tlv(&mut wire, 3, &packet(1, &[]));
            tlv(&mut wire, 4, &end(0, submitted, 1, peak));
            complete(&mut wire, u64::from(submitted), 1);
            assert_eq!(
                decode(request, retention, &wire).is_ok(),
                peak == expected,
                "submitted={submitted}, peak={peak}, expected={expected}"
            );
        }
    }
    Ok(())
}

#[test]
fn end_checks_counts_eos_and_peak_range_including_unused_reserved_slots() -> TestResult {
    let (request, retention) = request()?;
    let minimum = retention.fixed_bytes() + retention.picture_bytes();
    let exact = retention.peak_bytes(1)?;
    assert_eq!(minimum, 6208);
    assert_eq!(exact, 6224);
    for peak in [minimum, minimum + 1, retention.budget()] {
        let mut wire = start();
        tlv(
            &mut wire,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        tlv(&mut wire, 3, &packet(1, &[]));
        tlv(&mut wire, 4, &end(0, 1, 1, peak));
        complete(&mut wire, 1, 1);
        assert!(decode(request, retention, &wire).is_err());
    }
    for (base, submitted, packets, peak, eos) in [
        (1, 1, 1, exact, true),
        (0, 0, 1, exact, true),
        (0, 3, 1, retention.bytes(), true),
        (0, 1, 0, exact, true),
        (0, 1, 2, exact, true),
        (0, 1, 1, exact - 1, true),
        (0, 1, 1, exact + 1, true),
        (0, 1, 1, retention.budget() + 1, true),
        (0, 1, 1, exact, false),
    ] {
        let mut wire = start();
        tlv(
            &mut wire,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        tlv(&mut wire, 3, &packet(u32::from(eos), &[1]));
        tlv(&mut wire, 4, &end(base, submitted, packets, peak));
        assert!(
            Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn job_totals_and_frame_and_wire_limits_apply_across_segments() -> TestResult {
    let (request, retention) = request()?;
    for (frames, segments) in [(0, 0), (0, 1), (1, 0), (1, 2), (2, 1), (u64::MAX, 1)] {
        let mut wire = start();
        segment(&mut wire, request, retention, 0, 1)?;
        complete(&mut wire, frames, segments);
        assert!(decode(request, retention, &wire).is_err());
    }
    let wire = valid_wire(request, retention)?;
    for output_limit in [18, 19, wire.len() as u64 - 1, wire.len() as u64] {
        let limited = Request {
            limits: Limits::new(retention.budget(), output_limit, 16, 1024)?,
            ..request
        };
        assert_eq!(
            decode(limited, retention, &wire).is_ok(),
            output_limit == wire.len() as u64
        );
    }
    for frames in [1, 2, 3] {
        let limited = Request {
            limits: Limits::new(retention.budget(), 1 << 20, frames, 1024)?,
            ..request
        };
        assert_eq!(decode(limited, retention, &wire).is_ok(), frames == 3);
    }
    Ok(())
}

#[test]
fn callback_failure_is_terminal_and_stops_before_the_next_record() -> TestResult {
    let (request, retention) = request()?;
    let wire = valid_wire(request, retention)?;
    let mut decoder = Decoder::new(request, retention)?;
    let mut calls = 0;
    let result = decoder.push(&wire, |_| {
        calls += 1;
        Err(Error::Invalid("sink refused"))
    });
    assert!(matches!(result, Err(Error::Invalid("sink refused"))));
    assert_eq!(calls, 1);
    assert!(
        decoder
            .push(&wire, |_| {
                calls += 1;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(calls, 1);
    assert!(decoder.finish().is_err());
    Ok(())
}

struct FailingIo;

impl Read for FailingIo {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::PermissionDenied))
    }
}

impl Write for FailingIo {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn non_eof_io_errors_preserve_their_classification() -> TestResult {
    let (request, _) = request()?;
    assert!(
        matches!(Request::read(&mut FailingIo), Err(Error::Io(error)) if error.kind() == io::ErrorKind::PermissionDenied)
    );
    assert!(
        matches!(request.write(&mut FailingIo), Err(Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe)
    );
    assert!(
        matches!(OutputWriter::new(FailingIo, "4.2.0", request.limits), Err(Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe)
    );
    Ok(())
}

#[test]
fn writer_roundtrips_and_enforces_whole_wire_and_packet_byte_limits() -> TestResult {
    let (request, retention) = request()?;
    let mut wire = Vec::new();
    {
        let mut writer = OutputWriter::new(&mut wire, "v4.2.0", request.limits)?;
        writer.begin(0, retention, request.settings.keyint())?;
        writer.packet(-73, -81, 0, 99, &[17, 25, 38])?;
        writer.packet(0, 0, 1, 0, &[])?;
        writer.end(0, 1, 2, retention.peak_bytes(1)?)?;
        writer.complete(1, 1)?;
        assert!(writer.complete(1, 1).is_err());
        assert!(
            writer
                .begin(1, retention, request.settings.keyint())
                .is_err()
        );
    }
    assert!(decode(request, retention, &wire).is_ok());
    assert!(OutputWriter::new(Vec::new(), "4.2.1", request.limits).is_err());
    assert!(Limits::new(retention.budget(), 17, 1, 1).is_err());
    let exact = Limits::new(retention.budget(), 28, 1, 1)?;
    let mut bytes = Vec::new();
    let mut writer = OutputWriter::new(&mut bytes, "4.2.0", exact)?;
    assert!(
        writer
            .begin(0, retention, request.settings.keyint())
            .is_err()
    );
    assert!(writer.packet(0, 0, 1, 0, &[]).is_err());
    assert_eq!(bytes.len(), 18);
    let mut writer = OutputWriter::new(Vec::new(), "4.2.0", request.limits)?;
    assert!(writer.packet(0, 0, 0, 0, &[0; 1025]).is_err());
    assert!(writer.complete(1, 1).is_err());
    Ok(())
}

fn failure_kind(error: &Error) -> (u8, u32) {
    match error {
        Error::Invalid(_) => (1, 0),
        Error::Unsupported(_) => (2, 0),
        Error::Limit(_) => (3, 0),
        Error::Native(code) => (4, *code),
        Error::Output(_) => (5, 0),
        Error::Io(_) => (6, 0),
    }
}

#[test]
fn typed_failures_work_before_version_and_throughout_an_active_job() -> TestResult {
    let (request, retention) = request()?;
    for error in [
        Error::Invalid("invalid"),
        Error::Unsupported("unsupported"),
        Error::Limit("limit"),
        Error::Native(0x8000_0001),
        Error::Output("output"),
        Error::Io(io::Error::from(io::ErrorKind::BrokenPipe)),
    ] {
        let mut first = Vec::new();
        protocol::write_failure(&mut first, &error, request.limits)?;
        assert_eq!(first.len(), 18);
        for stage in 0..4 {
            let mut wire = Vec::new();
            if stage == 0 {
                wire.extend_from_slice(&first);
            } else {
                let mut writer = OutputWriter::new(&mut wire, "4.2.0", request.limits)?;
                if stage >= 2 {
                    writer.begin(0, retention, request.settings.keyint())?;
                }
                if stage == 3 {
                    writer.packet(0, 0, 1, 0, &[])?;
                    writer.end(0, 1, 1, retention.peak_bytes(1)?)?;
                }
                writer.failure(&error)?;
                assert!(writer.complete(1, 1).is_err());
                assert!(writer.failure(&error).is_err());
            }
            for split in 0..=wire.len() {
                let mut decoder = Decoder::new(request, retention)?;
                let (a, b) = wire.split_at(split);
                let result = decoder
                    .push(a, |_| Ok(()))
                    .and_then(|()| decoder.push(b, |_| Ok(())));
                let actual = result.err().ok_or("failure accepted")?;
                assert_eq!(failure_kind(&actual), failure_kind(&error));
                assert!(decoder.push(&[0], |_| Ok(())).is_err());
                assert!(decoder.finish().is_err());
            }
            wire.extend_from_slice(&[0; 100]);
            let actual = Decoder::new(request, retention)?
                .push(&wire, |_| Ok(()))
                .err()
                .ok_or("failure accepted")?;
            assert_eq!(failure_kind(&actual), failure_kind(&error));
        }
    }
    Ok(())
}

#[test]
fn malformed_failures_and_failures_after_job_end_are_rejected() -> TestResult {
    let (request, retention) = request()?;
    for length in [0, 1, 4, 6, u32::MAX] {
        let mut wire = protocol::OUTPUT_MAGIC.to_vec();
        wire.push(6);
        wire.extend_from_slice(&length.to_le_bytes());
        assert!(matches!(
            Decoder::new(request, retention)?.push(&wire, |_| Ok(())),
            Err(Error::Output(_))
        ));
    }
    for payload in [
        [0, 0, 0, 0, 0],
        [7, 0, 0, 0, 0],
        [255, 0, 0, 0, 0],
        [1, 1, 0, 0, 0],
        [3, 0, 0, 0, 1],
        [6, 42, 0, 0, 0],
    ] {
        let mut wire = protocol::OUTPUT_MAGIC.to_vec();
        tlv(&mut wire, 6, &payload);
        assert!(matches!(
            Decoder::new(request, retention)?.push(&wire, |_| Ok(())),
            Err(Error::Output(_))
        ));
    }
    let mut wire = valid_wire(request, retention)?;
    tlv(&mut wire, 6, &[3, 0, 0, 0, 0]);
    assert!(matches!(
        decode(request, retention, &wire),
        Err(Error::Output(_))
    ));
    let mut writer = OutputWriter::new(Vec::new(), "4.2.0", request.limits)?;
    writer.complete(1, 1)?;
    assert!(writer.failure(&Error::Limit("late failure")).is_err());
    assert!(Limits::new(retention.budget(), 17, 1, 1).is_err());
    Ok(())
}

#[test]
fn packet_capacity_checks_native_copy_before_allocation_at_the_exact_wire_boundary() -> TestResult {
    let (request, retention) = request()?;
    for (output_bytes, expected) in [
        (56, None),
        (57, Some(0)),
        (58, Some(1)),
        (1081, Some(1024)),
        (1082, Some(1024)),
    ] {
        let limits = Limits::new(retention.budget(), output_bytes, 1, 1024)?;
        let mut wire = Vec::new();
        let mut writer = OutputWriter::new(&mut wire, "4.2.0", limits)?;
        match expected {
            None => assert!(matches!(writer.packet_capacity(), Err(Error::Limit(_)))),
            Some(capacity) => {
                assert_eq!(writer.packet_capacity()?, capacity);
                writer.packet(0, 0, 1, 0, &vec![0; capacity])?;
                assert!(matches!(writer.packet_capacity(), Err(Error::Limit(_))));
            }
        }
        assert!(wire.len() as u64 <= output_bytes);
    }
    let mut writer = OutputWriter::new(Vec::new(), "4.2.0", request.limits)?;
    writer.complete(1, 1)?;
    assert!(writer.packet_capacity().is_err());
    Ok(())
}

#[test]
fn constructor_checks_version_and_failure_reserve_before_writing_any_bytes() -> TestResult {
    let (request, retention) = request()?;
    for output_bytes in 0..18 {
        assert!(Limits::new(retention.budget(), output_bytes, 1, 1).is_err());
    }
    for (version, minimum) in [("4.2.0", 28), ("v4.2.0", 29)] {
        for output_bytes in 18..=minimum {
            let limits = Limits::new(retention.budget(), output_bytes, 1, 1)?;
            let mut wire = Vec::new();
            let result = OutputWriter::new(&mut wire, version, limits);
            if output_bytes < minimum {
                assert!(matches!(result, Err(Error::Limit(_))));
                assert!(wire.is_empty());
                protocol::write_failure(&mut wire, &Error::Limit("initialization budget"), limits)?;
                assert_eq!(wire.len(), 18);
            } else {
                let mut writer = result?;
                writer.failure(&Error::Limit("no more room"))?;
                assert_eq!(wire.len() as u64, minimum);
            }
            let limited = Request { limits, ..request };
            assert!(matches!(
                decode(limited, retention, &wire),
                Err(Error::Limit(_))
            ));
        }
    }
    let mut wire = Vec::new();
    assert!(OutputWriter::new(&mut wire, "4.2.1", request.limits).is_err());
    assert!(wire.is_empty());
    Ok(())
}

#[test]
fn exhausted_packet_end_and_complete_budgets_leave_one_typed_failure_record() -> TestResult {
    let (request, retention) = request()?;
    for (operation, output_bytes) in [(0, 101), (1, 102), (2, 134), (3, 145), (4, 104)] {
        let limits = Limits::new(
            retention.budget(),
            output_bytes,
            1,
            if operation == 4 { 1 } else { 1024 },
        )?;
        let mut wire = Vec::new();
        let mut expected = start();
        tlv(
            &mut expected,
            2,
            &begin(0, retention, request.settings.keyint()),
        );
        {
            let mut writer = OutputWriter::new(&mut wire, "4.2.0", limits)?;
            writer.begin(0, retention, request.settings.keyint())?;
            if matches!(operation, 2 | 3) {
                writer.packet(0, 0, 1, 0, &[])?;
                let mut eos = Vec::new();
                eos.extend_from_slice(&0i64.to_le_bytes());
                eos.extend_from_slice(&0i64.to_le_bytes());
                eos.extend_from_slice(&1u32.to_le_bytes());
                eos.extend_from_slice(&0u32.to_le_bytes());
                tlv(&mut expected, 3, &eos);
            }
            if operation == 3 {
                writer.end(0, 1, 1, retention.peak_bytes(1)?)?;
                tlv(&mut expected, 4, &end(0, 1, 1, retention.peak_bytes(1)?));
            }
            let error = match operation {
                0 => writer.packet_capacity().map(|_| ()),
                1 => writer.packet(0, 0, 0, 0, &[42]),
                2 => writer.end(0, 1, 1, retention.peak_bytes(1)?),
                3 => writer.complete(1, 1),
                _ => writer.packet(0, 0, 0, 0, &[42, 43]),
            }
            .err()
            .ok_or("budget unexpectedly accepted")?;
            assert!(matches!(error, Error::Limit(_)));
            assert!(writer.packet_capacity().is_err());
            assert!(writer.packet(0, 0, 1, 0, &[]).is_err());
            assert!(
                writer
                    .begin(0, retention, request.settings.keyint())
                    .is_err()
            );
            assert!(writer.end(0, 1, 1, retention.peak_bytes(1)?).is_err());
            assert!(writer.complete(1, 1).is_err());
            writer.failure(&error)?;
            assert!(writer.failure(&error).is_err());
            assert!(writer.packet(0, 0, 0, 0, &[0; 1025]).is_err());
            assert!(writer.failure(&error).is_err());
        }
        tlv(&mut expected, 6, &[3, 0, 0, 0, 0]);
        assert_eq!(wire, expected, "operation {operation}");
        assert!(wire.len() as u64 <= output_bytes);
        assert!(matches!(
            decode(Request { limits, ..request }, retention, &wire),
            Err(Error::Limit(_))
        ));
    }
    Ok(())
}

#[test]
fn job_end_can_consume_the_failure_reserve_only_when_it_completes_successfully() -> TestResult {
    let (request, retention) = request()?;
    let limits = Limits::new(retention.budget(), 146, 1, 1024)?;
    let mut wire = Vec::new();
    {
        let mut writer = OutputWriter::new(&mut wire, "4.2.0", limits)?;
        writer.begin(0, retention, request.settings.keyint())?;
        writer.packet(0, 0, 1, 0, &[])?;
        writer.end(0, 1, 1, retention.peak_bytes(1)?)?;
        writer.complete(1, 1)?;
        assert!(writer.packet(0, 0, 0, 0, &[0; 1025]).is_err());
        assert!(writer.failure(&Error::Limit("late")).is_err());
    }
    assert_eq!(wire.len(), 146);
    assert_eq!(
        decode(Request { limits, ..request }, retention, &wire)?.last(),
        Some(&Record::Complete {
            frames: 1,
            segments: 1
        })
    );
    Ok(())
}
