#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;
use coreav_isobmff::Error;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn bounds() -> tstd::Bounds {
    tstd::Bounds {
        rx: 18_000_000,
        rb: 12_000_000,
        eb: 12_000_000,
        mb: 64_000,
    }
}

fn units(count: usize) -> Vec<Au> {
    (0..count)
        .map(|index| Au {
            pts90: 126_000 + index as u64 * 3_000 + if index % 2 == 0 { 0 } else { 3_000 },
            dts90: 126_000 + index as u64 * 3_000,
            idr: index % 12 == 0,
            data: (0..350).map(|byte| (byte + index) as u8).collect(),
        })
        .collect()
}

fn verify(bytes: &[u8], units: &[Au], bounds: tstd::Bounds) -> Result<(), ExtractError> {
    tstd::verify(
        bytes,
        units,
        bounds,
        &PlanningBudget::new(1 << 20),
        &mut Work {
            used: 0,
            max: 100_000,
        },
    )
}

fn serialized(units: &[Au]) -> Vec<u8> {
    let bytes = wire::serialize(
        units,
        bounds(),
        &Limits::default(),
        &PlanningBudget::new(8 << 20),
        &mut Work {
            used: 0,
            max: 100_000,
        },
    )
    .unwrap();
    assert_eq!(verify(&bytes, units, bounds()), Ok(()));
    bytes
}

fn reason(result: Result<(), ExtractError>, expected: &'static str) {
    assert_eq!(result, Err(unsupported(expected)));
}

fn pid(packet: &[u8]) -> u16 {
    (u16::from(packet[1] & 31) << 8) | u16::from(packet[2])
}

fn payload_at(packet: &[u8]) -> usize {
    if packet[3] & 0x20 != 0 {
        5 + usize::from(packet[4])
    } else {
        4
    }
}

fn video_start(bytes: &[u8], both: bool) -> usize {
    bytes
        .chunks_exact(188)
        .position(|packet| {
            pid(packet) == 256
                && packet[1] & 0x40 != 0
                && packet[payload_at(packet) + 7] == if both { 0xc0 } else { 0x80 }
        })
        .unwrap()
        * 188
}

fn pcr(packet: &[u8]) -> u64 {
    let p = &packet[6..12];
    let base = (u64::from(p[0]) << 25)
        | (u64::from(p[1]) << 17)
        | (u64::from(p[2]) << 9)
        | (u64::from(p[3]) << 1)
        | u64::from(p[4] >> 7);
    base * 300 + (u64::from(p[4] & 1) << 8) + u64::from(p[5])
}

fn set_pcr(packet: &mut [u8], ticks: u64) {
    let base = ticks / 300;
    let ext = ticks % 300;
    packet[6..12].copy_from_slice(&[
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        ((base & 1) << 7) as u8 | 0x7e | (ext >> 8) as u8,
        ext as u8,
    ]);
}

fn anchors(bytes: &[u8]) -> Vec<usize> {
    bytes
        .chunks_exact(188)
        .enumerate()
        .filter_map(|(index, packet)| {
            (packet[3] & 0x20 != 0 && packet[4] >= 7 && packet[5] & 0x10 != 0)
                .then_some(index * 188)
        })
        .collect()
}

fn null(packet: &mut [u8]) {
    packet.fill(0xff);
    packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
}

fn remove_tables(bytes: &mut [u8], table_pid: u16) {
    for packet in bytes.chunks_exact_mut(188) {
        if pid(packet) == table_pid {
            null(packet);
        }
    }
}

#[test]
fn serialized_payload_headers_and_continuity_are_verified() {
    let units = units(24);
    let base = serialized(&units);
    let first = video_start(&base, false);
    let both = video_start(&base, true);
    let payload = first + payload_at(&base[first..first + 188]);
    let dual = both + payload_at(&base[both..both + 188]);
    let continuation = base
        .chunks_exact(188)
        .enumerate()
        .find(|(_, packet)| pid(packet) == 256 && packet[3] & 0x10 != 0 && packet[1] & 0x40 == 0)
        .unwrap()
        .0
        * 188;
    for (offset, mask, expected) in [
        (0, 1, "invalid transport sync"),
        (1, 0x80, "unqualified transport header"),
        (3, 0x40, "unqualified transport header"),
        (first + 2, 1, "unexpected transport PID"),
        (continuation + 3, 1, "transport payload continuity"),
        (payload + 3, 1, "invalid qualified video PES header"),
        (payload + 4, 1, "invalid qualified video PES header"),
        (payload + 7, 0x40, "PES timestamp mismatch"),
        (payload + 8, 1, "PES timestamp mismatch"),
        (payload + 9, 1, "PES timestamp mismatch"),
        (payload + 11, 1, "PES timestamp mismatch"),
        (dual + 14, 1, "PES timestamp mismatch"),
        (dual + 18, 2, "PES timestamp mismatch"),
        (
            payload + 14,
            1,
            "serialized AVC payload differs from preparation",
        ),
        (first + 5, 0x40, "AVC transport random-access indication"),
        (188 + 4, 1, "PSI pointer or unit start"),
        (188 + 5, 1, "invalid PSI CRC or section header"),
        (188 + 21, 1, "invalid PSI CRC or section header"),
    ] {
        let mut changed = base.clone();
        changed[offset] ^= mask;
        reason(verify(&changed, &units, bounds()), expected);
    }
    for offset in [3, first + 3] {
        let mut changed = base.clone();
        changed[offset] &= 0xcf;
        reason(
            verify(&changed, &units, bounds()),
            "missing transport adaptation control",
        );
    }
    let mut changed = base.clone();
    changed[4] = 182;
    reason(
        verify(&changed, &units, bounds()),
        "invalid adaptation extent",
    );
    let mut changed = base.clone();
    changed[12] = 0;
    reason(
        verify(&changed, &units, bounds()),
        "invalid adaptation stuffing",
    );
    let mut changed = base.clone();
    changed[3] ^= 1;
    reason(
        verify(&changed, &units, bounds()),
        "adaptation-only continuity or signaling",
    );
}

#[test]
fn adaptation_with_payload_requires_at_least_one_payload_byte() {
    let units = units(3);
    let mut bytes = serialized(&units);
    bytes[3] |= 0x10;
    reason(
        verify(&bytes, &units, bounds()),
        "invalid adaptation extent",
    );
}

#[test]
fn pcr_clocks_and_actual_serialized_positions_bound_arrival_rate() {
    let units = units(3);
    let base = serialized(&units);
    let positions = anchors(&base);
    let next = positions[1];
    let first_ticks = pcr(&base[..188]);
    for ticks in [
        first_ticks - 1,
        first_ticks,
        first_ticks + tstd::PCR_GAP + 1,
    ] {
        let mut changed = base.clone();
        set_pcr(&mut changed[next..next + 188], ticks);
        reason(
            verify(&changed, &units, bounds()),
            "PCR repetition or range",
        );
    }
    let mut changed = base.clone();
    set_pcr(&mut changed[next..next + 188], first_ticks + 1);
    reason(
        verify(&changed, &units, bounds()),
        "transport-buffer arrival rate",
    );
    let delta = pcr(&base[next..next + 188]) - first_ticks;
    let inserted = (u128::from(bounds().rx) * u128::from(delta) / (188 * 8 * u128::from(tstd::F)))
        as usize
        + 1;
    let mut padding = vec![0; inserted * 188];
    for packet in padding.chunks_exact_mut(188) {
        null(packet);
    }
    let mut changed = base.clone();
    changed.splice(next..next, padding);
    reason(
        verify(&changed, &units, bounds()),
        "transport-buffer arrival rate",
    );
    for (high, low) in [(0, 0), (0x7f, 44), (0x7f, 0xff)] {
        let mut changed = base.clone();
        changed[10] = high;
        changed[11] = low;
        reason(verify(&changed, &units, bounds()), "invalid PCR syntax");
    }
    let mut changed = base.clone();
    changed[2] = 1;
    reason(
        verify(&changed, &units, bounds()),
        "PCR on an unexpected PID",
    );
}

#[test]
fn initial_terminal_anchors_and_all_program_tables_are_required() {
    let units = units(3);
    let base = serialized(&units);
    for (start, end) in [(188, base.len()), (0, base.len() - 188)] {
        reason(
            verify(&base[start..end], &units, bounds()),
            "transport payload lacks PCR brackets",
        );
    }
    for index in [0, base.len() - 188] {
        let mut changed = base.clone();
        changed[index + 5] = 0;
        changed[index + 6..index + 12].fill(0xff);
        reason(
            verify(&changed, &units, bounds()),
            "transport payload lacks PCR brackets",
        );
    }
    for table_pid in [0, 4096, 17] {
        let mut changed = base.clone();
        remove_tables(&mut changed, table_pid);
        reason(
            verify(&changed, &units, bounds()),
            if table_pid == 17 {
                "incomplete serialized transport"
            } else {
                "PES before complete initial system tables"
            },
        );
    }
    reason(
        verify(&[], &units, bounds()),
        "incomplete transport packets",
    );
    reason(
        verify(&base[..base.len() - 1], &units, bounds()),
        "incomplete transport packets",
    );
    reason(verify(&base, &units[..2], bounds()), "extra video PES");
    let extra = self::units(4);
    reason(
        verify(&base, &extra, bounds()),
        "incomplete serialized transport",
    );
}

#[test]
fn table_semantics_spacing_and_terminal_repetition_are_verified() {
    let units = units(3);
    let base = serialized(&units);
    for table_pid in [0, 4096, 17] {
        let at = base
            .chunks_exact(188)
            .position(|packet| pid(packet) == table_pid)
            .unwrap()
            * 188;
        let mut changed = base.clone();
        changed[at + 1] &= !0x40;
        reason(
            verify(&changed, &units, bounds()),
            "PSI pointer or unit start",
        );
        let mut changed = base.clone();
        let section = at + 5;
        let len =
            3 + (usize::from(changed[section + 1] & 15) << 8) + usize::from(changed[section + 2]);
        changed[section + 8] ^= 1;
        let crc = coreav_mpegts::crc32(&changed[section..section + len - 4]);
        changed[section + len - 4..section + len].copy_from_slice(&crc.to_be_bytes());
        reason(
            verify(&changed, &units, bounds()),
            "unqualified transport program metadata",
        );
    }
    let pat = base
        .chunks_exact(188)
        .position(|packet| pid(packet) == 0)
        .unwrap()
        * 188;
    let sdt = base
        .chunks_exact(188)
        .position(|packet| pid(packet) == 17)
        .unwrap()
        * 188;
    let pmt = base
        .chunks_exact(188)
        .position(|packet| pid(packet) == 4096)
        .unwrap()
        * 188;
    let mut changed = base.clone();
    changed[sdt..sdt + 188].copy_from_slice(&base[pat..pat + 188]);
    changed[sdt + 3] = (changed[sdt + 3] & 0xf0) | 1;
    reason(
        verify(&changed, &units, bounds()),
        "transport table repetition",
    );
    let mut changed = base.clone();
    changed[sdt..sdt + 188].copy_from_slice(&base[pmt..pmt + 188]);
    null(&mut changed[pmt..pmt + 188]);
    reason(
        verify(&changed, &units, bounds()),
        "transport system-buffer spacing",
    );
    let mut changed = base.clone();
    let mut terminal = base[base.len() - 188..].to_vec();
    let start = pcr(&terminal);
    for index in 1..=12 {
        set_pcr(&mut terminal, start + index * tstd::F / 20);
        changed.extend_from_slice(&terminal);
    }
    reason(
        verify(&changed, &units, bounds()),
        "missing final transport table repetition",
    );
}

#[test]
fn serialized_verification_enforces_elementary_and_multiplex_capacity_and_deadlines() {
    let units = units(3);
    let base = serialized(&units);
    let mut small = bounds();
    small.eb = units[0].data.len() as u64 * 8 + 7;
    reason(
        verify(&base, &units, small),
        "AVC elementary-buffer reservation",
    );
    small.eb = units
        .iter()
        .map(|unit| unit.data.len() as u64 * 8)
        .sum::<u64>()
        + 8;
    assert_eq!(verify(&base, &units, small), Ok(()));
    small.eb -= 1;
    reason(
        verify(&base, &units, small),
        "AVC elementary-buffer reservation",
    );
    small = bounds();
    small.mb = 2 * tstd::B;
    reason(
        verify(&base, &units, small),
        "MPEG-TS multiplex-buffer bound",
    );
    let mut changed = base.clone();
    for offset in anchors(&base) {
        let ticks = pcr(&base[offset..offset + 188]);
        set_pcr(&mut changed[offset..offset + 188], ticks + 2 * tstd::F);
    }
    reason(verify(&changed, &units, bounds()), "AVC transport delay");
    let first = video_start(&base, false);
    let byte = first + payload_at(&base[first..first + 188]);
    let positions = anchors(&base);
    let pair = positions
        .windows(2)
        .find(|pair| pair[0] + 10 <= byte && byte < pair[1] + 10)
        .unwrap();
    let left = pcr(&base[pair[0]..pair[0] + 188]);
    let right = pcr(&base[pair[1]..pair[1] + 188]);
    let arrival =
        left + ((byte - pair[0] - 10) as u64 * (right - left)) / (pair[1] - pair[0]) as u64;
    let shift = units[0].dts90 * 300 - arrival - 1;
    let mut changed = base.clone();
    for offset in positions {
        set_pcr(
            &mut changed[offset..offset + 188],
            pcr(&base[offset..offset + 188]) + shift,
        );
    }
    reason(
        verify(&changed, &units, bounds()),
        "AVC data misses its decode deadline",
    );
    small = bounds();
    small.rb = 1;
    small.mb = u64::MAX;
    reason(
        verify(&base, &units, small),
        "MPEG-TS multiplex-buffer bound",
    );
}

#[test]
fn verifier_work_and_allocation_ceilings_are_inclusive() {
    let units = units(3);
    let base = serialized(&units);
    let planning = PlanningBudget::new(1 << 20);
    let mut work = Work {
        used: 0,
        max: 100_000,
    };
    tstd::verify(&base, &units, bounds(), &planning, &mut work).unwrap();
    assert_eq!(work.used, 2 * (base.len() / 188) as u64);
    let charged = planning.used();
    assert!(charged > 0);
    assert_eq!(
        tstd::verify(
            &base,
            &units,
            bounds(),
            &PlanningBudget::new(charged),
            &mut Work {
                used: 0,
                max: work.used
            }
        ),
        Ok(())
    );
    assert_eq!(
        tstd::verify(
            &base,
            &units,
            bounds(),
            &PlanningBudget::new(charged - 1),
            &mut Work {
                used: 0,
                max: work.used
            }
        ),
        Err(limit(LimitKind::PlanningBytes))
    );
    for max in [0, (base.len() / 188) as u64, work.used - 1] {
        assert_eq!(
            tstd::verify(
                &base,
                &units,
                bounds(),
                &PlanningBudget::new(charged),
                &mut Work { used: 0, max }
            ),
            Err(limit(LimitKind::SampleWork))
        );
    }
}

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn generated(bframes: bool) -> Option<&'static [u8]> {
    static PLAIN: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    static REORDERED: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let cache = if bframes { &REORDERED } else { &PLAIN };
    cache
        .get_or_init(|| {
            let default = PathBuf::from("/opt/coreav-oracle-bin/ffmpeg");
            let ffmpeg = std::env::var_os("COREAV_FFMPEG")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    if default.is_file() {
                        return default.clone();
                    }
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .map(|home| home.join(".local/share/coreav-encoders/ffmpeg"))
                        .filter(|local| local.is_file())
                        .unwrap_or(default)
                });
            if !ffmpeg.is_file() {
                assert!(
                    std::env::var_os("CI").is_none()
                        && std::env::var_os("COREAV_REQUIRE_FFMPEG").is_none(),
                    "the pinned black-box FFmpeg is required"
                );
                return None;
            }
            let pin = include_str!("../../../../scripts/oracle-ffmpeg.env");
            let build = pin
                .lines()
                .find_map(|line| line.strip_prefix("BUILD="))
                .unwrap();
            let version = Command::new(&ffmpeg).arg("-version").output().unwrap();
            assert!(version.status.success());
            assert!(
                String::from_utf8_lossy(&version.stdout)
                    .starts_with(&format!("ffmpeg version {build}"))
            );
            let fixture = Fixture(
                std::env::temp_dir()
                    .join(format!("coreav-ts-{}-{bframes}.mp4", std::process::id())),
            );
            let mut child = Command::new(ffmpeg)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=64x64:rate=30",
                    "-frames:v",
                    "18",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-threads",
                    "1",
                    "-profile:v",
                    "high",
                    "-level:v",
                    "3.0",
                    "-x264-params",
                    if bframes {
                        "bframes=2:keyint=12:min-keyint=12:scenecut=0"
                    } else {
                        "bframes=0:keyint=12:min-keyint=12:scenecut=0"
                    },
                    "-use_editlist",
                    "0",
                    "-video_track_timescale",
                    "90000",
                ])
                .arg(&fixture.0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("fixture generation exceeded 30 seconds");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Some(std::fs::read(&fixture.0).unwrap())
        })
        .as_deref()
}

fn prepared(input: &[u8], limits: &Limits) -> Result<Prepared, ExtractError> {
    prepare(SliceSource::new(input), 0, limits)
}

fn fixture_box(input: &[u8], kind: &[u8; 4]) -> std::ops::Range<usize> {
    let matches: Vec<_> = input
        .windows(4)
        .enumerate()
        .filter_map(|(at, bytes)| (bytes == kind).then_some(at))
        .collect();
    assert_eq!(matches.len(), 1);
    let at = matches[0];
    let len = u32::from_be_bytes(input[at - 4..at].try_into().unwrap()) as usize;
    assert!(len >= 8 && at - 4 + len <= input.len());
    at + 4..at - 4 + len
}

fn fixture_packets(input: &[u8]) -> Vec<coreav_isobmff::Packet> {
    let limits = coreav_isobmff::Limits::default();
    let movie =
        coreav_isobmff::probe(SliceSource::new(input), &limits, &Allowlist::default()).unwrap();
    assert!(movie.tracks[0].tables_valid);
    let mut budget = Budget::new(&limits);
    movie
        .packets(0, SliceSource::new(input), &limits, &mut budget)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn preparation_refuses_noncanonical_avc_configuration_and_parameter_set_headers() {
    let Some(input) = generated(false) else {
        return;
    };
    prepared(input, &Limits::default()).unwrap();
    let avcc = fixture_box(input, b"avcC");
    let limits = coreav_isobmff::Limits::default();
    let movie =
        coreav_isobmff::probe(SliceSource::new(input), &limits, &Allowlist::default()).unwrap();
    assert_eq!(
        movie.tracks[0]
            .description
            .codec_config
            .as_ref()
            .unwrap()
            .data,
        input[avcc.clone()]
    );
    let config = &input[avcc.clone()];
    assert_eq!(&config[..6], &[1, 100, 0, 30, 0xff, 0xe1]);
    let sps = 8;
    let sps_len = usize::from(u16::from_be_bytes(config[6..8].try_into().unwrap()));
    assert_eq!(config[sps + sps_len], 1);
    let pps = sps + sps_len + 3;
    let pps_len = usize::from(u16::from_be_bytes(config[pps - 2..pps].try_into().unwrap()));
    let tail = pps + pps_len;
    assert_eq!(&config[tail..], &[0xfd, 0xf8, 0xf8, 0]);
    for (offset, value) in [
        (0, 0),
        (1, 101),
        (2, 1),
        (3, 31),
        (4, 0x7f),
        (4, 0xfc),
        (4, 0xfd),
        (4, 0xfe),
        (5, 0xc1),
        (sps, config[sps] | 0x80),
        (sps, config[sps] & !0x60),
        (pps, config[pps] | 0x80),
        (pps, config[pps] & !0x60),
        (tail, 0xfc),
        (tail, 0x7d),
        (tail + 1, 0xf9),
        (tail + 2, 0xf9),
        (tail + 3, 1),
    ] {
        let mut changed = input.to_vec();
        changed[avcc.start + offset] = value;
        let movie =
            coreav_isobmff::probe(SliceSource::new(&changed), &limits, &Allowlist::default())
                .unwrap();
        assert!(movie.tracks[0].tables_valid);
        assert_eq!(
            movie.tracks[0]
                .description
                .codec_config
                .as_ref()
                .unwrap()
                .data,
            changed[avcc.clone()]
        );
        assert!(
            prepared(&changed, &Limits::default()).is_err(),
            "accepted avcC byte {offset}={value}"
        );
    }
}

#[test]
fn preparation_refuses_parser_readable_cadence_and_composition_mutations() {
    for reordered in [false, true] {
        let Some(input) = generated(reordered) else {
            return;
        };
        prepared(input, &Limits::default()).unwrap();
        let baseline = fixture_packets(input);
        assert_eq!(baseline.len(), 18);
        assert!(baseline.iter().all(|packet| packet.duration == 3_000));
        let stts = fixture_box(input, b"stts");
        assert_eq!(
            &input[stts.start..stts.start + 12],
            &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 18]
        );
        let mut changed = input.to_vec();
        changed[stts.start + 12..stts.start + 16].copy_from_slice(&3_001u32.to_be_bytes());
        let packets = fixture_packets(&changed);
        assert_eq!(packets.len(), baseline.len());
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(packet.duration, 3_001);
            assert_eq!(packet.dts, index as i64 * 3_001);
            assert_eq!(
                (packet.offset, packet.size),
                (baseline[index].offset, baseline[index].size)
            );
        }
        assert_eq!(
            prepared(&changed, &Limits::default()).err(),
            Some(unsupported(
                "MPEG-TS copy requires qualified constant cadence"
            ))
        );
        if !reordered {
            continue;
        }
        assert!(baseline.iter().any(|packet| packet.cts != packet.dts));
        let ctts = fixture_box(input, b"ctts");
        assert_eq!(&input[ctts.start..ctts.start + 4], &[0, 0, 0, 0]);
        let count =
            u32::from_be_bytes(input[ctts.start + 8..ctts.start + 12].try_into().unwrap()) as usize;
        assert!((1..baseline.len()).contains(&count));
        let offset =
            u32::from_be_bytes(input[ctts.start + 12..ctts.start + 16].try_into().unwrap());
        for shift in [1, 3_000] {
            let mut changed = input.to_vec();
            changed[ctts.start + 12..ctts.start + 16]
                .copy_from_slice(&(offset + shift).to_be_bytes());
            let packets = fixture_packets(&changed);
            assert_eq!(packets.len(), baseline.len());
            for (index, (packet, original)) in packets.iter().zip(&baseline).enumerate() {
                assert_eq!(packet.dts, original.dts);
                assert_eq!(packet.duration, original.duration);
                assert_eq!(
                    (packet.offset, packet.size),
                    (original.offset, original.size)
                );
                assert_eq!(
                    packet.cts,
                    original.cts + if index < count { i64::from(shift) } else { 0 }
                );
            }
            assert!(
                prepared(&changed, &Limits::default()).is_err(),
                "accepted ctts shift {shift}"
            );
        }
    }
}

#[test]
fn preparation_rejects_first_and_late_malformed_samples_before_sealing() {
    for reordered in [false, true] {
        let Some(input) = generated(reordered) else {
            return;
        };
        let output = prepared(input, &Limits::default()).unwrap();
        let mut bytes = Vec::new();
        output.write_to(&mut bytes).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(bytes.len() % 188, 0);
        let iso_limits = coreav_isobmff::Limits::default();
        let movie =
            coreav_isobmff::probe(SliceSource::new(input), &iso_limits, &Allowlist::default())
                .unwrap();
        let mut budget = Budget::new(&iso_limits);
        let packets: Vec<_> = movie
            .packets(0, SliceSource::new(input), &iso_limits, &mut budget)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(packets.len(), 18);
        for packet in [&packets[0], packets.last().unwrap()] {
            let offset = packet.offset as usize;
            for replacement in [[0, 0, 0, 0], u32::MAX.to_be_bytes()] {
                let mut changed = input.to_vec();
                changed[offset..offset + 4].copy_from_slice(&replacement);
                assert!(prepared(&changed, &Limits::default()).is_err());
            }
            let mut changed = input.to_vec();
            changed[offset + 4] |= 0x80;
            assert!(prepared(&changed, &Limits::default()).is_err());
        }
        assert!(prepare(SliceSource::new(input), 1, &Limits::default()).is_err());
    }
}

#[test]
fn preparation_limits_refuse_instead_of_returning_partial_output() {
    let Some(input) = generated(false) else {
        return;
    };
    let normal = prepared(input, &Limits::default()).unwrap();
    let exact = Limits {
        max_input_bytes: input.len(),
        max_output_bytes: normal.bytes.len(),
        max_samples: 18,
        ..Limits::default()
    };
    assert!(prepared(input, &exact).is_ok());
    for limits in [
        Limits {
            max_input_bytes: input.len() - 1,
            ..exact
        },
        Limits {
            max_output_bytes: normal.bytes.len() - 1,
            ..exact
        },
        Limits {
            max_samples: 17,
            ..exact
        },
        Limits {
            max_planning_bytes: 0,
            ..exact
        },
        Limits {
            max_table_bytes: 0,
            ..exact
        },
        Limits {
            max_metadata_bytes: 0,
            ..exact
        },
        Limits {
            max_access_unit_bytes: 0,
            ..exact
        },
        Limits {
            max_nals_per_sample: 0,
            ..exact
        },
        Limits {
            max_nals: 0,
            ..exact
        },
        Limits {
            max_work: 0,
            ..exact
        },
    ] {
        assert!(prepared(input, &limits).is_err(), "accepted {limits:?}");
    }
    assert!(prepared(&[], &Limits::default()).is_err());
    assert!(prepared(&input[..input.len() - 1], &Limits::default()).is_err());
}

#[test]
fn preparation_nal_and_converted_access_unit_limits_include_the_boundary() {
    let Some(input) = generated(false) else {
        return;
    };
    let normal = prepared(input, &Limits::default()).unwrap();
    let iso_limits = coreav_isobmff::Limits::default();
    let movie =
        coreav_isobmff::probe(SliceSource::new(input), &iso_limits, &Allowlist::default()).unwrap();
    let mut budget = Budget::new(&iso_limits);
    let mut total = 0;
    let mut maximum = 0;
    for packet in movie
        .packets(0, SliceSource::new(input), &iso_limits, &mut budget)
        .unwrap()
    {
        let packet = packet.unwrap();
        let mut data =
            &input[packet.offset as usize..packet.offset as usize + packet.size as usize];
        let mut count = 0;
        while !data.is_empty() {
            let len = u32::from_be_bytes(data[..4].try_into().unwrap()) as usize;
            assert!(len > 0);
            data = &data[4 + len..];
            count += 1;
        }
        maximum = maximum.max(count);
        total += count;
    }
    let mut current = 0;
    let mut converted_max = 0;
    for packet in normal.bytes.chunks_exact(188) {
        if pid(packet) != 256 || packet[3] & 0x10 == 0 {
            continue;
        }
        let payload = &packet[payload_at(packet)..];
        if packet[1] & 0x40 != 0 {
            converted_max = converted_max.max(current);
            current = payload.len() - 9 - usize::from(payload[8]);
        } else {
            current += payload.len();
        }
    }
    converted_max = converted_max.max(current);
    let exact = Limits {
        max_access_unit_bytes: converted_max,
        max_nals: total as u64,
        max_nals_per_sample: maximum,
        ..Limits::default()
    };
    assert_eq!(prepared(input, &exact).unwrap().bytes, normal.bytes);
    for (limits, kind) in [
        (
            Limits {
                max_access_unit_bytes: converted_max - 1,
                ..exact
            },
            LimitKind::PacketBytes,
        ),
        (
            Limits {
                max_nals: total as u64 - 1,
                ..exact
            },
            LimitKind::SampleWork,
        ),
        (
            Limits {
                max_nals_per_sample: maximum - 1,
                ..exact
            },
            LimitKind::SampleWork,
        ),
    ] {
        assert_eq!(prepared(input, &limits).err(), Some(limit(kind)));
    }
}

#[test]
fn preparation_planning_table_metadata_and_work_ceiling_edges_preserve_output() {
    let Some(input) = generated(false) else {
        return;
    };
    let normal = prepared(input, &Limits::default()).unwrap();
    type SetLimit = fn(&mut Limits, usize);
    let setters: [(usize, SetLimit); 4] = [
        (Limits::default().max_planning_bytes, |limits, value| {
            limits.max_planning_bytes = value
        }),
        (
            Limits::default().max_table_bytes as usize,
            |limits, value| limits.max_table_bytes = value as u64,
        ),
        (
            Limits::default().max_metadata_bytes as usize,
            |limits, value| limits.max_metadata_bytes = value as u64,
        ),
        (Limits::default().max_work as usize, |limits, value| {
            limits.max_work = value as u64
        }),
    ];
    for (mut high, set) in setters {
        let mut low = 0;
        let mut limits = Limits::default();
        set(&mut limits, low);
        assert!(prepared(input, &limits).is_err());
        while low + 1 < high {
            let middle = low + (high - low) / 2;
            set(&mut limits, middle);
            if prepared(input, &limits).is_ok() {
                high = middle;
            } else {
                low = middle;
            }
        }
        set(&mut limits, high);
        assert_eq!(prepared(input, &limits).unwrap().bytes, normal.bytes);
        set(&mut limits, high - 1);
        assert!(matches!(
            prepared(input, &limits),
            Err(ExtractError::Source(Error::LimitExceeded { .. }))
        ));
    }
}

struct CapturedSource {
    bytes: Vec<u8>,
    reads: usize,
    lengths: usize,
    fail: bool,
    change_length: bool,
}

impl Source for CapturedSource {
    fn len(&mut self) -> Result<u64, Error> {
        self.lengths += 1;
        Ok(self.bytes.len() as u64 + u64::from(self.change_length && self.lengths > 1))
    }

    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        if self.fail {
            return Err(Error::Truncated { offset });
        }
        SliceSource::new(&self.bytes).read_exact_at(offset, out)
    }
}

#[test]
fn sealed_preparation_owns_bytes_and_propagates_capture_and_sink_failures() {
    let Some(input) = generated(false) else {
        return;
    };
    let mut source = CapturedSource {
        bytes: input.to_vec(),
        reads: 0,
        lengths: 0,
        fail: false,
        change_length: false,
    };
    let sealed = prepare(&mut source, 0, &Limits::default()).unwrap();
    assert_eq!((source.reads, source.lengths), (1, 2));
    source.bytes.fill(0);
    source.fail = true;
    let mut first = Vec::new();
    let mut second = Vec::new();
    sealed.write_to(&mut first).unwrap();
    sealed.write_to(&mut second).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, sealed.bytes);
    assert_eq!((source.reads, source.lengths), (1, 2));
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        sealed.write_to(Broken).unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert!(matches!(
        prepare(&mut source, 0, &Limits::default()),
        Err(ExtractError::Source(Error::Truncated { .. }))
    ));
    source.bytes = input.to_vec();
    source.fail = false;
    source.lengths = 0;
    source.change_length = true;
    assert!(matches!(
        prepare(source, 0, &Limits::default()),
        Err(ExtractError::SourceChanged)
    ));
}
