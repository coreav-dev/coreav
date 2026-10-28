//! Probe metadata: profile and level, bit rate, codec
//! configuration bytes and creation times, on small files built from the
//! specifications, plus hostile and limit cases.

#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

mod common;

use common::*;
use coreav_isobmff::{
    Allowlist, Codec, DeclaredBitRate, Error, FourCc, LimitKind, Limits, ProResProfile, Profile,
    Rational, SliceSource, Warning, probe, unix_time,
};

/// Creation times written by [`one_track`]: movie, track, media.
const CREATED: (u64, u64, u64) = (3_786_912_000, 3_786_912_001, 3_786_912_002);

/// A one-track movie with a given sample entry and handler. Headers use
/// version `version` (ISO/IEC 14496-12 8.2.2, 8.3.2, 8.4.2) and carry
/// [`CREATED`]; four 100-byte samples of 512 ticks at 12,800 Hz.
fn one_track(entry: Vec<u8>, handler: &[u8; 4], version: u8) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mdat = bx(b"mdat", &[0xAB; 400]);
    let chunk_offset = (ftyp.len() + 8) as u32;
    let times = |created: u64, timescale: Option<u32>, id: Option<u32>, duration: u32| {
        let mut out = Vec::new();
        if version == 1 {
            out.extend_from_slice(&created.to_be_bytes());
            out.extend_from_slice(&created.to_be_bytes());
        } else {
            out.extend_from_slice(&(created as u32).to_be_bytes());
            out.extend_from_slice(&(created as u32).to_be_bytes());
        }
        if let Some(id) = id {
            out.extend_from_slice(&u32s(&[id, 0]));
        }
        if let Some(timescale) = timescale {
            out.extend_from_slice(&timescale.to_be_bytes());
        }
        if version == 1 {
            out.extend_from_slice(&u64::from(duration).to_be_bytes());
        } else {
            out.extend_from_slice(&duration.to_be_bytes());
        }
        out
    };
    let mut mvhd = times(CREATED.0, Some(MOVIE_TIMESCALE), None, 160);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = times(CREATED.1, None, Some(1), 160);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[
        0x10000,
        0,
        0,
        0,
        0x10000,
        0,
        0,
        0,
        0x4000_0000,
        0,
        0,
    ]));
    let mut mdhd = times(CREATED.2, Some(TIMESCALE), None, 2048);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(handler);
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
    let mut stsz = u32s(&[0, 4]);
    stsz.extend_from_slice(&u32s(&[100, 90, 110, 100]));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[1, 4, 512])),
            full(b"stsc", 0, 0, &u32s(&[1, 1, 4, 1])),
            full(b"stsz", 0, 0, &stsz),
            full(b"stco", 0, 0, &u32s(&[1, chunk_offset])),
        ]),
    );
    let minf = bx(b"minf", &stbl);
    let mdia = bx(
        b"mdia",
        &cat(&[
            full(b"mdhd", version, 0, &mdhd),
            full(b"hdlr", 0, 0, &hdlr),
            minf,
        ]),
    );
    let trak = bx(b"trak", &cat(&[full(b"tkhd", version, 3, &tkhd), mdia]));
    let moov = bx(b"moov", &cat(&[full(b"mvhd", version, 0, &mvhd), trak]));
    cat(&[ftyp, mdat, moov])
}

fn probe_ok(data: &[u8]) -> coreav_isobmff::Movie {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap()
}

fn video(kind: &[u8; 4], config: Vec<u8>) -> coreav_isobmff::Track {
    probe_ok(&one_track(visual_entry(kind, &config), b"vide", 0))
        .tracks
        .remove(0)
}

/// An `mp4a` entry whose `esds` carries `config` as `DecoderSpecificInfo`
/// and the given buffer size, maximum and average bit rates, plus `extra`
/// child boxes.
fn mp4a(config: &[u8], rates: (u32, u32, u32), extra: &[u8]) -> Vec<u8> {
    let mut decoder = vec![0x40, 0x15];
    decoder.extend_from_slice(&rates.0.to_be_bytes()[1..]);
    decoder.extend_from_slice(&rates.1.to_be_bytes());
    decoder.extend_from_slice(&rates.2.to_be_bytes());
    decoder.extend_from_slice(&[5, config.len() as u8]);
    decoder.extend_from_slice(config);
    let mut es = vec![0, 0, 0, 4, decoder.len() as u8];
    es.extend_from_slice(&decoder);
    let mut payload = vec![3, es.len() as u8];
    payload.extend_from_slice(&es);
    let mut body = audio_entry(b"mp4a").split_off(8);
    body.extend_from_slice(&full(b"esds", 0, 0, &payload));
    body.extend_from_slice(extra);
    bx(b"mp4a", &body)
}

#[test]
fn h264_profile_and_level_from_avcc() {
    let track = video(b"avc1", bx(b"avcC", &[1, 100, 0, 31, 0xff, 0xe0, 0]));
    assert_eq!(
        track.description.profile,
        Some(Profile::H264 {
            profile_idc: 100,
            constraint_flags: 0,
            level_idc: 31
        })
    );
    let config = track.description.codec_config.unwrap();
    assert_eq!(config.kind, FourCc(*b"avcC"));
    assert_eq!(config.data, vec![1, 100, 0, 31, 0xff, 0xe0, 0]);
    assert!(track.warnings.is_empty(), "{:?}", track.warnings);
}

#[test]
fn h264_sps_wins_over_a_conflicting_header() {
    let avcc = [1, 66, 0, 41, 0xff, 0xe1, 0, 4, 0x67, 66, 0xc0, 30, 1, 0];
    let track = video(b"avc1", bx(b"avcC", &avcc));
    assert_eq!(
        track.description.profile,
        Some(Profile::H264 {
            profile_idc: 66,
            constraint_flags: 0xc0,
            level_idc: 30
        })
    );
    assert!(
        track
            .warnings
            .contains(&Warning::MalformedCodecConfig(FourCc(*b"avcC")))
    );
}

#[test]
fn hevc_av1_and_vp9_profiles() {
    let mut hvcc = vec![1, 0x22, 0x20, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120];
    hvcc.extend_from_slice(&[0; 10]);
    let track = video(b"hvc1", bx(b"hvcC", &hvcc));
    assert_eq!(
        track.description.profile,
        Some(Profile::Hevc {
            profile_space: 0,
            high_tier: true,
            profile_idc: 2,
            compatibility_flags: 0x2000_0000,
            constraint_flags: 0x9000_0000_0000,
            level_idc: 120
        })
    );
    let track = video(b"av01", bx(b"av1C", &[0x81, 0x48, 0x8c, 0]));
    assert_eq!(
        track.description.profile,
        Some(Profile::Av1 {
            profile: 2,
            level_idx: 8,
            high_tier: true,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false
        })
    );
    let vpcc = [1, 0, 0, 0, 2, 31, 0xa2, 1, 1, 1, 0, 0];
    let track = video(b"vp09", full(b"vpcC", 1, 0, &vpcc[4..]));
    assert_eq!(
        track.description.profile,
        Some(Profile::Vp9 {
            profile: 2,
            level: 31,
            bit_depth: 10
        })
    );
    assert_eq!(track.description.color.full_range, Some(false));
}

#[test]
fn prores_profile_and_no_configuration() {
    let track = video(b"apch", Vec::new());
    assert_eq!(track.description.codec, Codec::ProRes);
    assert_eq!(
        track.description.profile,
        Some(Profile::ProRes(ProResProfile::Hq))
    );
    assert_eq!(track.description.codec_config, None);
}

#[test]
fn aac_profile_and_declared_bit_rate() {
    // LC with an esds decoder configuration and no btrt.
    let data = av_movie_with(mp4a(&[0x12, 0x10], (512, 160_000, 128_000), &[]), false);
    let movie = probe_ok(&data);
    let audio = &movie.tracks[1].description;
    assert_eq!(audio.codec, Codec::Aac);
    assert_eq!(
        audio.profile,
        Some(Profile::Aac {
            object_type: 2,
            sbr: false,
            ps: false
        })
    );
    assert_eq!(
        audio.declared_bit_rate,
        Some(DeclaredBitRate {
            source: FourCc(*b"esds"),
            average: 128_000,
            maximum: 160_000,
            buffer_size: 512
        })
    );
    assert_eq!(audio.codec_config.as_ref().unwrap().kind, FourCc(*b"esds"));
    // HE-AAC v2 by explicit hierarchical signalling (object type 29), and a
    // btrt that takes precedence over the esds values.
    let btrt = bx(b"btrt", &u32s(&[1024, 96_000, 64_000]));
    let he2 = [0xeb, 0x09, 0x88, 0x00];
    let data = av_movie_with(mp4a(&he2, (0, 0, 0), &btrt), false);
    let movie = probe_ok(&data);
    let audio = &movie.tracks[1].description;
    assert_eq!(
        audio.profile,
        Some(Profile::Aac {
            object_type: 2,
            sbr: true,
            ps: true
        })
    );
    assert_eq!(
        audio.declared_bit_rate,
        Some(DeclaredBitRate {
            source: FourCc(*b"btrt"),
            average: 64_000,
            maximum: 96_000,
            buffer_size: 1024
        })
    );
}

#[test]
fn malformed_configuration_is_a_warning() {
    // A wrong configurationVersion: no profile, a warning, the file probes.
    let track = video(b"avc1", bx(b"avcC", &[0, 100, 0, 31]));
    assert_eq!(track.description.profile, None);
    assert!(
        track
            .warnings
            .contains(&Warning::MalformedCodecConfig(FourCc(*b"avcC")))
    );
    // An AAC configuration shorter than its fixed fields.
    let data = av_movie_with(mp4a(&[0x12], (0, 0, 0), &[]), false);
    let movie = probe_ok(&data);
    assert_eq!(movie.tracks[1].description.profile, None);
    assert!(
        movie.tracks[1]
            .warnings
            .contains(&Warning::MalformedCodecConfig(FourCc(*b"esds")))
    );
}

#[test]
fn exact_bit_rate_from_sample_sizes() {
    // Four samples of 100 + 90 + 110 + 100 bytes over mdhd 2,048 ticks at
    // 12,800 Hz (0.16 s): 400 × 8 / 0.16 = 20,000 bit/s.
    let movie = probe_ok(&one_track(
        visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 31, 0xff, 0xe0, 0])),
        b"vide",
        0,
    ));
    let track = &movie.tracks[0];
    assert_eq!(track.sample_bytes, Some(400));
    assert_eq!(track.bit_rate(), Some(Rational::from_int(20_000)));
    // Constant sample size: 4 × 100 bytes, same duration.
    let movie = probe_ok(&av_movie(b"sowt"));
    assert_eq!(movie.tracks[1].sample_bytes, Some(400));
    assert_eq!(movie.tracks[1].bit_rate(), Some(Rational::from_int(20_000)));
    // Default spec: 280 bytes over the media edit of 1,024 ticks (80 ms).
    let movie = probe_ok(&movie_bytes());
    assert_eq!(movie.tracks[0].sample_bytes, Some(280));
    assert_eq!(movie.tracks[0].bit_rate(), Some(Rational::from_int(28_000)));
}

fn movie_bytes() -> Vec<u8> {
    movie(&Spec::default())
}

#[test]
fn fragmented_bit_rate_needs_a_scan() {
    let data = fragmented_movie(None, None);
    let limits = Limits::default();
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    // Probe never reads fragments, so the bytes are unknown.
    assert_eq!(movie.tracks[0].sample_bytes, None);
    assert_eq!(movie.tracks[0].bit_rate(), None);
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    // Four 100-byte samples of 512 ticks.
    let track = &movie.tracks[0];
    assert_eq!(track.sample_bytes, Some(400));
    let expected = Rational::new(400 * 8 * i128::from(track.timescale), 2048).unwrap();
    assert_eq!(track.bit_rate(), Some(expected));
}

#[test]
fn creation_times_in_both_versions() {
    for version in [0, 1] {
        let movie = probe_ok(&one_track(
            visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 31, 0xff, 0xe0, 0])),
            b"vide",
            version,
        ));
        let track = &movie.tracks[0];
        assert_eq!(
            (
                movie.creation_time,
                track.creation_time,
                track.media_creation_time
            ),
            CREATED,
            "version {version}"
        );
        assert_eq!(track.timescale, TIMESCALE);
        assert_eq!(track.mdhd_duration, 2048);
        assert_eq!(movie.mvhd_duration, 160);
    }
    // 3,786,912,000 s after 1904-01-01 is 2024-01-01 00:00:00 UTC.
    assert_eq!(unix_time(CREATED.0), Some(1_704_067_200));
    assert_eq!(unix_time(0), None);
    assert_eq!(unix_time(u64::MAX), None);
    assert_eq!(unix_time(1), Some(1 - 2_082_844_800));
}

#[test]
fn codec_configuration_counts_against_retained_bytes() {
    let mut avcc = vec![1, 100, 0, 31, 0xff, 0xe0, 0];
    avcc.resize(4096, 0);
    let data = one_track(visual_entry(b"avc1", &bx(b"avcC", &avcc)), b"vide", 0);
    let limits = Limits {
        max_retained_bytes: 1024,
        ..Limits::default()
    };
    assert!(matches!(
        probe(SliceSource::new(&data), &limits, &Allowlist::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::RetainedBytes,
            ..
        })
    ));
    let movie = probe_ok(&data);
    assert_eq!(
        movie.tracks[0]
            .description
            .codec_config
            .as_ref()
            .unwrap()
            .data
            .len(),
        4096
    );
}

#[test]
fn summing_sample_sizes_is_bounded_by_the_work_limit() {
    // stts, stsc (one entry each) and four listed stsz entries: a budget of
    // five visits is exceeded by the size sum, never by the validation.
    let data = one_track(
        visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 31, 0xff, 0xe0, 0])),
        b"vide",
        0,
    );
    let limits = Limits {
        max_sample_work: 5,
        ..Limits::default()
    };
    assert!(matches!(
        probe(SliceSource::new(&data), &limits, &Allowlist::default()),
        Err(Error::LimitExceeded {
            limit: LimitKind::SampleWork,
            ..
        })
    ));
    let limits = Limits {
        max_sample_work: 6,
        ..Limits::default()
    };
    assert!(probe(SliceSource::new(&data), &limits, &Allowlist::default()).is_ok());
}

#[test]
fn every_truncation_of_the_configuration_probes_or_fails_cleanly() {
    let configs = [
        bx(
            b"avcC",
            &[1, 66, 0, 41, 0xff, 0xe1, 0, 4, 0x67, 66, 0xc0, 30],
        ),
        bx(b"hvcC", &[1, 0x22, 0x20, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 120]),
        bx(b"av1C", &[0x81, 0x48, 0x8c, 0, 0x0a, 2, 0x18, 0x28]),
        full(b"vpcC", 1, 0, &[2, 31, 0xa2, 1, 1, 1, 0, 0]),
        bx(b"btrt", &u32s(&[1, 2, 3])),
    ];
    for config in configs {
        for end in 0..config.len() {
            // Shorten the payload but keep a consistent box size.
            let mut cut = config[..end.max(8)].to_vec();
            let size = (cut.len() as u32).to_be_bytes();
            cut[..4].copy_from_slice(&size);
            let data = one_track(visual_entry(b"avc1", &cut), b"vide", 0);
            let result = probe(
                SliceSource::new(&data),
                &Limits::default(),
                &Allowlist::default(),
            );
            assert!(result.is_ok(), "{:?}", result.err());
        }
    }
}

/// Rewrites the version 0 `tfdt` of `fragmented_movie` as version 1 with
/// `value`, fixing the enclosing sizes and the `trun` data offset.
fn with_tfdt(data: &[u8], value: i64) -> Vec<u8> {
    let find = |data: &[u8], kind: &[u8; 4]| data.windows(4).position(|w| w == kind).unwrap() - 4;
    let tfdt = find(data, b"tfdt");
    let mut out = data[..tfdt].to_vec();
    out.extend_from_slice(&full(b"tfdt", 1, 0, &value.to_be_bytes()));
    out.extend_from_slice(&data[tfdt + 16..]);
    for kind in [b"moof", b"traf"] {
        let at = find(&out, kind);
        let size = u32::from_be_bytes(out[at..at + 4].try_into().unwrap()) + 4;
        out[at..at + 4].copy_from_slice(&size.to_be_bytes());
    }
    let trun = find(&out, b"trun");
    let offset = u32::from_be_bytes(out[trun + 16..trun + 20].try_into().unwrap()) + 4;
    out[trun + 16..trun + 20].copy_from_slice(&offset.to_be_bytes());
    out
}

#[test]
fn byte_count_survives_a_huge_fragment_decode_time() {
    let limits = Limits::default();
    // Sanity: the rewritten file with a small tfdt still sums its bytes.
    let data = with_tfdt(&fragmented_movie(None, None), 7);
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].sample_bytes, Some(400));
    // tfdt = i64::MAX: decode times overflow while walking runs. The scan
    // still succeeds (as the duration-only scan did); the bytes are unknown.
    let data = with_tfdt(&fragmented_movie(None, None), i64::MAX);
    let mut movie = probe(SliceSource::new(&data), &limits, &Allowlist::default()).unwrap();
    movie
        .scan_fragments(SliceSource::new(&data), &limits)
        .unwrap();
    assert_eq!(movie.tracks[0].sample_bytes, None);
    assert_eq!(movie.tracks[0].bit_rate(), None);
    assert_eq!(movie.tracks[0].sample_duration, 2048);
}
