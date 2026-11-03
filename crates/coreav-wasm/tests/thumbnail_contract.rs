//! Linux-only synthetic thumbnail contracts and input-bound allocation witnesses.
#![cfg(all(target_os = "linux", feature = "native-h264"))]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path};

use coreav_counting_alloc::{CountingAllocator, measure};
use coreav_isobmff::{BufferedSource, FileSource, Rational, SliceSource, Source};
use coreav_thumbnail::{ThumbnailOptions, oracle_choice};
use coreav_wasm::ops::{self, Failure};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[path = "../../coreav-mkv/tests/common/mod.rs"]
mod mkv;
#[path = "../../coreav-isobmff/tests/common/mod.rs"]
mod mp4;
#[path = "../../coreav-h264/tests/common/streams.rs"]
mod streams;

use streams::{Writer, idr_pcm_slice, non_idr_pcm_slice_ref, pcm_samples, pps, sps_with_max_refs};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn nal(reference: u8, kind: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut out = streams::nal(reference, kind, Vec::new());
    let mut zeros = 0;
    for byte in payload {
        if zeros == 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn sps(a: u32, b: u32, id: u32, crop: Option<[u32; 4]>) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 51).ue(id);
    w.ue(0)
        .ue(0)
        .ue(2)
        .ue(2)
        .flag(false)
        .ue(a - 1)
        .ue(b - 1)
        .flag(true)
        .flag(true)
        .flag(crop.is_some());
    if let Some(crop) = crop {
        for value in crop {
            w.ue(value);
        }
    }
    w.flag(false);
    nal(3, 7, w.finish())
}

fn pps_for(id: u32, sequence: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(id)
        .ue(sequence)
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
    nal(3, 8, w.finish())
}

fn dc_slice(first: u32, macroblocks: usize, picture: u32, predicted: bool) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(first)
        .ue(if predicted { 5 } else { 7 })
        .ue(picture)
        .u(4, 0)
        .ue(0)
        .u(6, 0);
    if predicted {
        w.flag(false).flag(false);
    }
    w.flag(false).flag(false).se(0);
    for _ in 0..macroblocks {
        w.ue(3).ue(0).se(0).u(1, 1);
    }
    nal(3, 5, w.finish())
}

fn good_packet() -> Vec<u8> {
    [
        nal(3, 7, sps_with_max_refs(2, 2, 2)),
        nal(3, 8, pps()),
        nal(3, 5, idr_pcm_slice(4, pcm_samples)),
    ]
    .concat()
}

fn units(bytes: &[u8]) -> Vec<&[u8]> {
    let starts: Vec<_> = bytes
        .windows(4)
        .enumerate()
        .filter_map(|(i, value)| (value == [0, 0, 0, 1]).then_some(i))
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, start)| &bytes[start + 4..starts.get(i + 1).copied().unwrap_or(bytes.len())])
        .collect()
}

fn framed(bytes: &[u8]) -> Vec<u8> {
    units(bytes)
        .into_iter()
        .flat_map(|unit| [(unit.len() as u32).to_be_bytes().to_vec(), unit.to_vec()].concat())
        .collect()
}

fn avcc(sequence: &[u8], picture: &[u8]) -> Vec<u8> {
    let sequence = &sequence[4..];
    let picture = &picture[4..];
    [
        vec![1, sequence[1], sequence[2], sequence[3], 0xff, 0xe1],
        (sequence.len() as u16).to_be_bytes().to_vec(),
        sequence.to_vec(),
        vec![1],
        (picture.len() as u16).to_be_bytes().to_vec(),
        picture.to_vec(),
    ]
    .concat()
}

fn entry(record: &[u8], width: u16, height: u16) -> Vec<u8> {
    let mut entry = mp4::visual_entry(b"avc1", &mp4::bx(b"avcC", record));
    entry[32..34].copy_from_slice(&width.to_be_bytes());
    entry[34..36].copy_from_slice(&height.to_be_bytes());
    entry
}

fn movie(entries: &[Vec<u8>], packets: &[(Vec<u8>, u32, bool)]) -> Vec<u8> {
    use mp4::{bx, cat, full, u32s};
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mdat = bx(
        b"mdat",
        &packets
            .iter()
            .flat_map(|(p, _, _)| p.clone())
            .collect::<Vec<_>>(),
    );
    let count = packets.len() as u32;
    let matrix = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];
    let mvhd = [
        u32s(&[0, 0, 1000, 40 * count]),
        vec![0; 16],
        u32s(&matrix),
        vec![0; 28],
    ]
    .concat();
    let tkhd = [
        u32s(&[0, 0, 1, 0, 40 * count]),
        vec![0; 16],
        u32s(&matrix),
        u32s(&[32 << 16, 32 << 16]),
    ]
    .concat();
    let mdhd = [u32s(&[0, 0, 1000, 40 * count]), vec![0x55, 0xc4, 0, 0]].concat();
    let hdlr = [
        u32s(&[0]),
        b"vide".to_vec(),
        vec![0; 12],
        b"VideoHandler\0".to_vec(),
    ]
    .concat();
    let mut offset = (ftyp.len() + 8) as u32;
    let mut offsets = Vec::new();
    let mut chunks = Vec::new();
    let mut sync = Vec::new();
    for (i, (packet, description, key)) in packets.iter().enumerate() {
        offsets.push(offset);
        offset += packet.len() as u32;
        chunks.extend([i as u32 + 1, 1, *description]);
        if *key {
            sync.push(i as u32 + 1);
        }
    }
    let sizes: Vec<_> = packets
        .iter()
        .map(|(packet, _, _)| packet.len() as u32)
        .collect();
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(
                b"stsd",
                0,
                0,
                &[u32s(&[entries.len() as u32]), cat(entries)].concat(),
            ),
            full(b"stts", 0, 0, &u32s(&[1, count, 40])),
            full(b"stsc", 0, 0, &[u32s(&[count]), u32s(&chunks)].concat()),
            full(b"stsz", 0, 0, &[u32s(&[0, count]), u32s(&sizes)].concat()),
            full(b"stco", 0, 0, &[u32s(&[count]), u32s(&offsets)].concat()),
            full(
                b"stss",
                0,
                0,
                &[u32s(&[sync.len() as u32]), u32s(&sync)].concat(),
            ),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(
        b"minf",
        &cat(&[full(b"vmhd", 0, 1, &[0; 8]), bx(b"dinf", &dref), stbl]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    cat(&[
        ftyp,
        mdat,
        bx(
            b"moov",
            &cat(&[
                full(b"mvhd", 0, 0, &mvhd),
                bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia])),
            ]),
        ),
    ])
}

fn matroska(packets: &[(Vec<u8>, bool, bool)], width: u64, height: u64, extra: &[u8]) -> Vec<u8> {
    let track = mkv::video_track(
        1,
        "V_MPEG4/ISO/AVC",
        width,
        height,
        &[mkv::uint(0x23_E383, 40_000_000), extra.to_vec()].concat(),
    );
    let blocks: Vec<_> = packets
        .iter()
        .enumerate()
        .map(|(i, (packet, key, invisible))| {
            let mut block = mkv::simple_block(1, i as i16 * 40, *key, packet);
            if *invisible {
                let flag = block.len() - packet.len() - 1;
                block[flag] |= 8;
            }
            block
        })
        .collect();
    mkv::file(
        "matroska",
        &[
            mkv::info(Some(packets.len() as f64 * 40.0)),
            mkv::el(mkv::TRACKS, &track),
            mkv::cluster(0, &blocks),
        ],
    )
}

fn file(packet: Vec<u8>) -> Vec<u8> {
    matroska(&[(packet, true, false)], 32, 32, &[])
}

fn options() -> Value {
    json!({"at":"0", "width":32, "format":"png"})
}

fn run(bytes: &[u8], options: &Value) -> (Result<Value, Failure>, Vec<u8>) {
    let mut output = Vec::new();
    let result = ops::run_stream(
        "thumbnail",
        SliceSource::new(bytes),
        &options.to_string(),
        &mut output,
    )
    .map(|document| serde_json::from_str(&document).expect("summary JSON"));
    (result, output)
}

fn error(bytes: &[u8], options: &Value) -> Value {
    refusal(run(bytes, options))
}

fn refusal((result, output): (Result<Value, Failure>, Vec<u8>)) -> Value {
    assert!(output.is_empty(), "a refusal wrote image bytes");
    match result {
        Err(Failure::Error(value)) => {
            assert_eq!(value["schema"], "coreav.error/1");
            value
        }
        other => panic!("expected a structured media refusal: {other:?}"),
    }
}

fn benign_reuse() {
    let (result, output) = run(&file(good_packet()), &options());
    let summary = result.expect("benign operation after refusal");
    assert_eq!(summary["output"]["width"], 32);
    assert_eq!(summary["output"]["height"], 32);
    assert_eq!(summary["packets_decoded"], 1);
    assert!(output.starts_with(b"\x89PNG\r\n\x1a\n"));
}

struct Case {
    id: &'static str,
    bytes: Vec<u8>,
    options: Value,
    kind: &'static str,
}

fn hostile() -> Vec<Case> {
    let good = good_packet();
    let configured =
        |sequence: Vec<u8>| [sequence, nal(3, 8, pps()), dc_slice(0, 1, 0, false)].concat();
    let small = sps(2, 2, 0, None);
    let record = avcc(&small, &nal(3, 8, pps()));
    let mut conflicting = good.clone();
    conflicting.extend(sps(3, 2, 0, None));
    let mut many_sets = good.clone();
    many_sets.splice(0..0, sps(2, 2, 32, None));
    let mut many_slices = [small.clone(), nal(3, 8, pps())].concat();
    for first in 0..5 {
        many_slices.extend(dc_slice(first, 1, 0, false));
    }
    let recovery = [
        small.clone(),
        nal(3, 8, pps()),
        nal(0, 6, vec![6, 1, 0xc0, 0x80]),
        nal(3, 1, non_idr_pcm_slice_ref(0, 0, 4, true)),
    ]
    .concat();
    let mut mixed = [small.clone(), nal(3, 8, pps()), dc_slice(0, 2, 0, false)].concat();
    mixed.extend(dc_slice(2, 2, 0, true));
    let mut multiple = good.clone();
    multiple.extend(dc_slice(0, 4, 0, false));
    let mut truncated = good.clone();
    truncated.truncate(truncated.len() - 100);
    let stripping = mkv::el(
        0x6D80,
        &mkv::el(
            0x6240,
            &mkv::cat(&[
                mkv::uint(0x5031, 0),
                mkv::uint(0x5032, 1),
                mkv::uint(0x5033, 0),
                mkv::el(
                    0x5034,
                    &mkv::cat(&[mkv::uint(0x4254, 3), mkv::el(0x4255, &good[..12])]),
                ),
            ]),
        ),
    );
    let change = movie(
        &[
            entry(&record, 32, 32),
            entry(&avcc(&sps(240, 135, 0, None), &nal(3, 8, pps())), 16, 16),
        ],
        &[
            (framed(&good), 1, true),
            (framed(&dc_slice(0, 1, 0, false)), 2, true),
        ],
    );
    let fallback = matroska(
        &[
            (good.clone(), true, true),
            (
                nal(3, 1, non_idr_pcm_slice_ref(1, 2, 4, true)),
                false,
                false,
            ),
        ],
        32,
        32,
        &[],
    );
    let mut first_without_idr = nal(3, 1, non_idr_pcm_slice_ref(0, 0, 4, true));
    first_without_idr.extend(nal(0, 12, vec![0xff; 9_000]));
    let fallback_listing = matroska(
        &[
            (first_without_idr, true, false),
            (nal(3, 1, non_idr_pcm_slice_ref(1, 2, 4, true)), true, false),
            (good.clone(), true, false),
        ],
        32,
        32,
        &[],
    );
    let mut cases = vec![
        Case {
            id: "sps-enormous",
            bytes: file(configured(sps(1024, 1024, 0, None))),
            options: options(),
            kind: "limit",
        },
        Case {
            id: "sps-overflow",
            bytes: file(configured(sps(1 << 28, 1 << 28, 0, None))),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "invalid-crop",
            bytes: file(configured(sps(2, 2, 0, Some([0, 17, 0, 0])))),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "empty-crop",
            bytes: file(configured(sps(2, 2, 0, Some([0, 16, 0, 0])))),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "sample-description-change",
            bytes: change,
            options: json!({"at":"0.04","width":32,"format":"png"}),
            kind: "limit",
        },
        Case {
            id: "conflicting-in-band-sps",
            bytes: file(conflicting),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "many-sets",
            bytes: file(many_sets),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "many-slices",
            bytes: file(many_slices),
            options: options(),
            kind: "limit",
        },
        Case {
            id: "recovery-non-idr",
            bytes: matroska(
                &[
                    (good.clone(), true, false),
                    (recovery, true, false),
                    (good.clone(), true, false),
                ],
                32,
                32,
                &[],
            ),
            options: json!({"at":"0.04","width":32,"format":"png"}),
            kind: "decode",
        },
        Case {
            id: "exact-fallback",
            bytes: fallback,
            options: options(),
            kind: "unsupported_feature",
        },
        Case {
            id: "mixed-slices",
            bytes: file(mixed),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "multiple-pictures",
            bytes: file(multiple),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "truncated-idr",
            bytes: file(truncated),
            options: options(),
            kind: "decode",
        },
        Case {
            id: "restored-packet-limit",
            bytes: matroska(&[(good[12..].to_vec(), true, false)], 32, 32, &stripping),
            options: json!({"at":"0","width":32,"limits":{"maxPacketBytes":good.len()-1}}),
            kind: "limit",
        },
        Case {
            id: "output-limit",
            bytes: file(good.clone()),
            options: json!({"at":"0","width":32,"format":"png","limits":{"maxOutputBytes":1}}),
            kind: "limit",
        },
        Case {
            id: "planning-fallback-limit",
            bytes: fallback_listing,
            options: json!({"at":"0","width":32,"limits":{"maxPlanningBytes":20_000}}),
            kind: "limit",
        },
        Case {
            id: "work-limit",
            bytes: movie(&[entry(&record, 32, 32)], &[(framed(&good), 1, true)]),
            options: json!({"at":"0","width":32,"limits":{"maxSampleWork":0}}),
            kind: "limit",
        },
    ];
    cases.extend(boundary_cases());
    cases
}

#[test]
fn synthetic_contract_roster_keeps_expected_outcomes_and_allows_reuse() {
    let registered: Value = serde_json::from_str(include_str!(
        "../../../packages/wasm-h264/test/installed/roster.json"
    ))
    .unwrap();
    let expected: BTreeSet<_> = ["hostile", "boundaries"]
        .into_iter()
        .flat_map(|key| registered[key].as_array().unwrap().iter())
        .map(|v| v.as_str().unwrap())
        .collect();
    let cases = hostile();
    assert_eq!(
        cases.iter().map(|case| case.id).collect::<BTreeSet<_>>(),
        expected
    );
    assert_eq!(cases.len(), expected.len());
    for case in cases {
        let (result, image) = run(&case.bytes, &case.options);
        let outcome = match &result {
            Ok(_) => "success",
            Err(Failure::Usage(_)) => "usage",
            Err(Failure::Error(value)) => match value["kind"].as_str() {
                Some("limit") => "limit",
                Some("decode") => "decode",
                Some("unsupported_feature") => "unsupported_feature",
                Some("io") => "io",
                _ => "other",
            },
        };
        eprintln!("thumbnail-contract step={} outcome={outcome}", case.id);
        if case.kind == "success" {
            let summary = result.unwrap_or_else(|error| panic!("{}: {error:?}", case.id));
            assert_eq!(summary["packets_decoded"], 1, "{}", case.id);
            assert!(!image.is_empty(), "{}", case.id);
        } else {
            let refused = refusal((result, image));
            assert_eq!(refused["kind"], case.kind, "{}: {refused}", case.id);
        }
        eprintln!("thumbnail-contract step={} outcome=benign-reuse", case.id);
        benign_reuse();
    }
}

#[test]
fn ordinary_mp4_and_matroska_match_the_same_bounded_native_operation() {
    let good = good_packet();
    let record = avcc(&sps(2, 2, 0, None), &nal(3, 8, pps()));
    for bytes in [
        file(good.clone()),
        movie(&[entry(&record, 32, 32)], &[(framed(&good), 1, true)]),
    ] {
        let (summary, image) = run(&bytes, &options());
        let summary = summary.unwrap();
        let mut native_options = ThumbnailOptions::at(Rational::from_int(0));
        native_options.width = Some(32);
        native_options.format = coreav_thumbnail::ImageFormat::Png;
        let native =
            coreav_thumbnail::thumbnail_bounded(SliceSource::new(&bytes), &native_options).unwrap();
        assert_eq!(image, native.image);
        assert_eq!(
            summary,
            coreav_cli::thumbnail::document(&native, native_options.at)
        );
    }
}

#[test]
fn selected_recovery_and_inter_frame_fallback_are_not_replaced_with_another_idr() {
    for case in hostile()
        .into_iter()
        .filter(|case| matches!(case.id, "recovery-non-idr" | "exact-fallback"))
    {
        let mut native = ThumbnailOptions::at(if case.id == "recovery-non-idr" {
            Rational::new(1, 25).unwrap()
        } else {
            Rational::from_int(0)
        });
        native.width = Some(32);
        let choice = oracle_choice(SliceSource::new(&case.bytes), &native).unwrap();
        if case.id == "recovery-non-idr" {
            assert_eq!(choice.time, Rational::new(1, 25));
        } else {
            assert_ne!(choice.start, choice.target);
        }
        assert_eq!(error(&case.bytes, &case.options)["kind"], case.kind);
    }
}

#[test]
fn aggregate_limit_is_observed_after_the_full_listing_fallback_starts() {
    struct Traced<'a> {
        inner: SliceSource<'a>,
        first_payload: u64,
        last_payload: u64,
        probes: usize,
        fallback: bool,
    }
    impl Source for Traced<'_> {
        fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
            self.inner.len()
        }
        fn read_exact_at(
            &mut self,
            offset: u64,
            bytes: &mut [u8],
        ) -> Result<(), coreav_isobmff::Error> {
            if offset == self.first_payload && bytes.len() == 8192 {
                self.probes += 1;
            }
            if self.probes > 0
                && offset < self.last_payload
                && offset.saturating_add(bytes.len() as u64) > self.last_payload - 4
            {
                self.fallback = true;
            }
            self.inner.read_exact_at(offset, bytes)
        }
    }
    let case = hostile()
        .into_iter()
        .find(|case| case.id == "planning-fallback-limit")
        .unwrap();
    let segment = coreav_mkv::probe(
        SliceSource::new(&case.bytes),
        &coreav_mkv::Limits::default(),
        &coreav_mkv::Allowlist::default(),
    )
    .unwrap();
    let mut source = SliceSource::new(&case.bytes);
    let packets: Vec<_> = segment
        .packets(&mut source, &coreav_mkv::Limits::default())
        .map(Result::unwrap)
        .collect();
    let native = oracle_choice(
        SliceSource::new(&case.bytes),
        &ThumbnailOptions::at(Rational::from_int(0)),
    )
    .unwrap();
    assert_eq!(native.target, packets[2].offset);
    let mut source = Traced {
        inner: SliceSource::new(&case.bytes),
        first_payload: packets[0].offset,
        last_payload: packets[2].offset,
        probes: 0,
        fallback: false,
    };
    let mut output = Vec::new();
    let result = ops::run_stream(
        "thumbnail",
        &mut source,
        &case.options.to_string(),
        &mut output,
    );
    let Err(Failure::Error(failure)) = result else {
        panic!("fallback budget must refuse");
    };
    assert_eq!(failure["kind"], "limit");
    assert!(output.is_empty());
    assert!(
        source.probes > 0,
        "the partial listing never checked its first packet"
    );
    assert!(
        source.fallback,
        "the budget failed before the fallback listing"
    );
    benign_reuse();
}

fn boundary_cases() -> Vec<Case> {
    let good = good_packet();
    let second = [
        sps(3, 2, 0, None),
        nal(3, 8, pps()),
        dc_slice(0, 6, 0, false),
    ]
    .concat();
    let changed = movie(
        &[
            entry(&avcc(&sps(2, 2, 0, None), &nal(3, 8, pps())), 32, 32),
            entry(&avcc(&sps(3, 2, 0, None), &nal(3, 8, pps())), 48, 32),
        ],
        &[(framed(&good), 1, true), (framed(&second), 2, true)],
    );
    let mut tables = Vec::new();
    for id in 0..32 {
        tables.extend(sps(2, 2, id, None));
    }
    for id in 0..256 {
        tables.extend(pps_for(id, 0));
    }
    tables.extend(dc_slice(0, 4, 0, false));
    let mut slices = [sps(120, 68, 0, Some([0, 0, 0, 4])), nal(3, 8, pps())].concat();
    for first in 0..120 * 68 {
        slices.extend(dc_slice(first, 1, 0, false));
    }
    assert_eq!(units(&slices).len(), 8_160 + 2);
    let mut partial = matroska(
        &[(good.clone(), true, false), (good, true, false)],
        32,
        32,
        &[],
    );
    partial.truncate(partial.len() - 100);
    vec![
        Case {
            id: "later-sample-description",
            bytes: changed,
            options: json!({"at":"0.04","width":32,"height":32,"format":"png"}),
            kind: "success",
        },
        Case {
            id: "legal-many-sets",
            bytes: file(tables),
            options: options(),
            kind: "success",
        },
        Case {
            id: "legal-many-slices",
            bytes: matroska(&[(slices, true, false)], 1920, 1080, &[]),
            options: options(),
            kind: "success",
        },
        Case {
            id: "partial-matroska",
            bytes: partial,
            options: options(),
            kind: "success",
        },
    ]
}

#[test]
fn activated_sps_not_container_or_first_unused_sps_authorizes_pixels() {
    let large = sps(240, 135, 1, None);
    let activated = [
        sps(2, 2, 0, None),
        large,
        pps_for(1, 1),
        dc_slice(0, 1, 1, false),
    ]
    .concat();
    let cropped = [
        sps(240, 135, 0, Some([0, 1912, 0, 1072])),
        nal(3, 8, pps()),
        dc_slice(0, 1, 0, false),
    ]
    .concat();
    for packet in [activated, cropped] {
        for (width, height) in [(3840, 2160), (16, 16)] {
            let bytes = matroska(&[(packet.clone(), true, false)], width, height, &[]);
            let ((result, output), counters) = measure(|| run(&bytes, &options()));
            assert!(output.is_empty());
            let Err(Failure::Error(failure)) = result else {
                panic!("geometry must be refused");
            };
            assert_eq!(failure["kind"], "limit", "{failure}");
            assert_eq!(counters.trough_bytes, 0);
            assert!(counters.bytes < 3840 * (2160 + 8));
            benign_reuse();
        }
    }
}

#[test]
#[ignore = "exports only synthetic fixtures on Linux for the installed-module harness"]
fn export_synthetic_contract_fixtures() {
    let directory =
        std::env::var_os("COREAV_THUMBNAIL_HOSTILE_DIR").expect("COREAV_THUMBNAIL_HOSTILE_DIR");
    let directory = Path::new(&directory);
    assert!(!directory.exists(), "export directory must be new");
    std::fs::create_dir_all(directory).unwrap();
    let mut manifest = Vec::new();
    for case in hostile() {
        let path = format!(
            "{}.{}",
            case.id,
            if case.bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
                "mkv"
            } else {
                "mp4"
            }
        );
        std::fs::write(directory.join(&path), &case.bytes).unwrap();
        manifest.push(json!({"id":case.id,"path":path,"sha256":format!("{:x}",Sha256::digest(&case.bytes)),"options":case.options,"expectation":case.kind}));
    }
    std::fs::write(
        directory.join("hostile.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let benign = file(good_packet());
    std::fs::write(directory.join("benign.mkv"), &benign).unwrap();
    std::fs::write(
        directory.join("benign.json"),
        serde_json::to_vec_pretty(&json!({
            "id":"benign", "path":"benign.mkv", "sha256":format!("{:x}",Sha256::digest(&benign)),
            "options":options(), "expectation":"success"
        }))
        .unwrap(),
    )
    .unwrap();
}

#[derive(Deserialize)]
struct Manifest {
    fixtures: Vec<Fixture>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    id: String,
    path: String,
    sha256: String,
    geometry: String,
    entropy: String,
    container: String,
    options: Value,
    coded_width: u32,
    coded_height: u32,
    visible_width: u32,
    visible_height: u32,
}

fn hash(path: &Path) -> String {
    let mut input = std::fs::File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65_536];
    loop {
        let n = input.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    format!("{:x}", digest.finalize())
}

#[test]
#[ignore = "requires the registered generated 4K manifest on Linux"]
fn generated_4k_refusals_allocate_no_decoded_frame() {
    let manifest_path =
        std::env::var_os("COREAV_THUMBNAIL_MANIFEST").expect("COREAV_THUMBNAIL_MANIFEST");
    let report_path = std::env::var_os("COREAV_THUMBNAIL_ALLOCATION_REPORT")
        .expect("COREAV_THUMBNAIL_ALLOCATION_REPORT");
    let manifest_path = Path::new(&manifest_path);
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let fixtures: Vec<_> = manifest
        .fixtures
        .iter()
        .filter(|row| matches!(row.geometry.as_str(), "4k" | "4k-small-container"))
        .collect();
    let expected: BTreeSet<_> = ["4k", "4k-small-container"]
        .into_iter()
        .flat_map(|geometry| {
            ["cabac", "cavlc"].into_iter().flat_map(move |entropy| {
                ["mp4", "matroska"]
                    .into_iter()
                    .map(move |container| format!("{geometry}-{entropy}-{container}"))
            })
        })
        .collect();
    assert_eq!(fixtures.len(), 8);
    assert_eq!(
        fixtures
            .iter()
            .map(|row| row.id.clone())
            .collect::<BTreeSet<_>>(),
        expected
    );
    let binary = std::env::current_exe().unwrap();
    let binary_sha = hash(&binary);
    let mut results = Vec::new();
    for fixture in fixtures {
        assert_eq!(
            fixture.id,
            format!(
                "{}-{}-{}",
                fixture.geometry, fixture.entropy, fixture.container
            )
        );
        assert_eq!(
            (
                fixture.coded_width,
                fixture.coded_height,
                fixture.visible_width,
                fixture.visible_height
            ),
            (3840, 2160, 3840, 2160)
        );
        assert_eq!(
            fixture.options,
            json!({"at":"1","width":320,"format":"jpeg"})
        );
        assert!(!fixture.path.is_empty());
        assert!(
            Path::new(&fixture.path)
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        );
        let path = manifest_path.parent().unwrap().join(&fixture.path);
        assert_eq!(hash(&path), fixture.sha256);
        let options = fixture.options.to_string();
        let (result, counters) = measure(|| {
            let source = BufferedSource::new(FileSource::open(&path).expect("fixture file"));
            ops::run_stream("thumbnail", source, &options, std::io::sink())
        });
        let Err(Failure::Error(failure)) = result else {
            panic!("{} was not an admission refusal", fixture.id);
        };
        assert_eq!(failure["kind"], "limit", "{}: {failure}", fixture.id);
        let minimum_luma = 3840u64 * (2160 + 8);
        assert_eq!(counters.trough_bytes, 0, "{}: {counters:?}", fixture.id);
        assert!(
            counters.bytes < minimum_luma,
            "{}: {counters:?}",
            fixture.id
        );
        assert_eq!(hash(&path), fixture.sha256);
        results.push(json!({"id":fixture.id,"path":fixture.path,"inputSha256":fixture.sha256,"options":fixture.options,
            "optionsSha256":format!("{:x}",Sha256::digest(options.as_bytes())),"binarySha256":binary_sha,
            "admissionRefused":true,"requestedBytes":counters.bytes,"minimumLumaAllocationBytes":minimum_luma,
            "troughBytes":counters.trough_bytes,"peakBytes":counters.peak_bytes,"allocations":counters.allocations,
            "reallocations":counters.reallocations,"liveBytes":counters.live_bytes,"error":failure}));
        benign_reuse();
    }
    assert_eq!(hash(&binary), binary_sha);
    std::fs::write(report_path,serde_json::to_vec_pretty(&json!({"schema":"coreav.thumbnail-allocations/1","testBinarySha256":binary_sha,"records":results})).unwrap()).unwrap();
}
