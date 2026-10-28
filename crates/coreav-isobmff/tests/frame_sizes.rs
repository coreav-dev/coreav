//! `Movie::scan_frame_sizes` and the parameter set sizes of codec
//! configurations (`docs/design/frame-sizes.md`): sample entries that
//! understate their size, parameter sets carried in samples, VP9 and ProRes
//! frame headers, and samples that cannot be walked.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::{bx, cat, full, u32s, visual_entry};
use coreav_isobmff::{Allowlist, Budget, Limits, Movie, SliceSource, probe};

/// Fields packed most significant bit first; `Ue` as Exp-Golomb.
#[derive(Clone, Copy)]
enum F {
    U(u64, u32),
    Ue(u32),
}

fn pack(fields: &[F], stop_bit: bool) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    let mut put = |value: u64, width: u32| {
        for i in (0..width).rev() {
            bits.push(value >> i & 1 == 1);
        }
    };
    for field in fields {
        match *field {
            F::U(value, width) => put(value, width),
            F::Ue(value) => {
                let code = u64::from(value) + 1;
                let len = 64 - code.leading_zeros();
                put(0, len - 1);
                put(code, len);
            }
        }
    }
    if stop_bit {
        bits.push(true);
    }
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    bits.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
        })
        .collect()
}

/// An H.264 Main profile SPS NAL unit for `width_mbs` × `height_mbs`
/// macroblocks, progressive, not cropped.
fn avc_sps(width_mbs: u32, height_mbs: u32) -> Vec<u8> {
    let mut nal = vec![0x67];
    nal.extend(pack(
        &[
            F::U(77, 8),
            F::U(0, 8),
            F::U(40, 8),
            F::Ue(0),
            F::Ue(0),
            F::Ue(2),
            F::Ue(1),
            F::U(0, 1),
            F::Ue(width_mbs - 1),
            F::Ue(height_mbs - 1),
            F::U(1, 1),
            F::U(1, 1),
            F::U(0, 1),
            F::U(0, 1),
        ],
        true,
    ));
    nal
}

/// An `avcC` record with 4-byte NAL lengths and these SPS.
fn avcc(sps: &[Vec<u8>]) -> Vec<u8> {
    let mut record = vec![1, 77, 0, 40, 0xff, 0xe0 | u8::try_from(sps.len()).unwrap()];
    for s in sps {
        record.extend(u16::try_from(s.len()).unwrap().to_be_bytes());
        record.extend(s);
    }
    record.push(0);
    record
}

/// A length-prefixed (4 bytes) sample of these NAL units.
fn nal_sample(nals: &[Vec<u8>]) -> Vec<u8> {
    nals.iter()
        .flat_map(|n| {
            let mut out = u32::try_from(n.len()).unwrap().to_be_bytes().to_vec();
            out.extend(n);
            out
        })
        .collect()
}

/// A movie with one video track (`vide` handler): a sample entry of type
/// `kind` declaring 320×240 with `config` as its children, and these
/// samples in one chunk.
fn movie(kind: &[u8; 4], config: &[u8], samples: &[Vec<u8>]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let data: Vec<u8> = samples.concat();
    let mdat = bx(b"mdat", &data);
    let offset = u32::try_from(ftyp.len() + 8).unwrap();
    let count = u32::try_from(samples.len()).unwrap();
    let mut mvhd = u32s(&[0, 0, 1000, count * 40]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = u32s(&[0, 0, 1, 0, count * 40]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    tkhd.extend_from_slice(&u32s(&[320 << 16, 240 << 16]));
    let mut mdhd = u32s(&[0, 0, 1000, count * 40]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0u8; 13]);
    let sizes: Vec<u32> = samples
        .iter()
        .map(|s| u32::try_from(s.len()).unwrap())
        .collect();
    let mut stsz = u32s(&[0, count]);
    stsz.extend(u32s(&sizes));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(
                b"stsd",
                0,
                0,
                &[u32s(&[1]), visual_entry(kind, config)].concat(),
            ),
            full(b"stts", 0, 0, &u32s(&[1, count, 40])),
            full(b"stsc", 0, 0, &u32s(&[1, 1, count, 1])),
            full(b"stsz", 0, 0, &stsz),
            full(b"stco", 0, 0, &u32s(&[1, offset])),
            full(b"stss", 0, 0, &u32s(&[1, 1])),
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
        &cat(&[full(b"vmhd", 0, 1, &[0u8; 8]), bx(b"dinf", &dref), stbl]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let trak = bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]));
    let moov = bx(b"moov", &cat(&[full(b"mvhd", 0, 0, &mvhd), trak]));
    cat(&[ftyp, mdat, moov])
}

fn probed(data: &[u8]) -> Movie {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap()
}

fn scanned(data: &[u8]) -> Option<(u32, u32)> {
    let movie = probed(data);
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    movie
        .scan_frame_sizes(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
}

const SLICE: [u8; 4] = [0x65, 0x88, 0x84, 0x00];

#[test]
fn an_understated_sample_entry_is_caught_by_the_configuration() {
    // The entry declares 320×240; the avcC's SPS is 7680×4320.
    let config = bx(b"avcC", &avcc(&[avc_sps(480, 270)]));
    let data = movie(b"avc1", &config, &[nal_sample(&[SLICE.to_vec()])]);
    let movie = probed(&data);
    let description = &movie.tracks[0].description;
    assert_eq!(
        (description.width, description.height),
        (Some(320), Some(240))
    );
    assert_eq!(description.parameter_set_size, Some((7680, 4320)));
    assert_eq!(scanned(&data), Some((7680, 4320)));
}

#[test]
fn parameter_sets_in_samples_raise_the_bound() {
    // avc1 with a 320×240 SPS in the record, and a 7680×4320 SPS in the
    // second sample, as a decoder would apply it.
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    let samples = [
        nal_sample(&[SLICE.to_vec()]),
        nal_sample(&[avc_sps(480, 270), SLICE.to_vec()]),
    ];
    let data = movie(b"avc1", &config, &samples);
    assert_eq!(
        probed(&data).tracks[0].description.parameter_set_size,
        Some((320, 240))
    );
    assert_eq!(scanned(&data), Some((7680, 4320)));
    // avc3 with no parameter set in the record: only the samples tell.
    let empty = bx(b"avcC", &avcc(&[]));
    let data = movie(
        b"avc3",
        &empty,
        &[nal_sample(&[avc_sps(40, 30), SLICE.to_vec()])],
    );
    assert_eq!(probed(&data).tracks[0].description.parameter_set_size, None);
    assert_eq!(scanned(&data), Some((640, 480)));
    // ...and with no SPS anywhere, no size is known.
    let data = movie(b"avc3", &empty, &[nal_sample(&[SLICE.to_vec()])]);
    assert_eq!(scanned(&data), None);
}

#[test]
fn zero_lengths_and_short_tails_hold_no_nal_unit() {
    // A zero-filled sample (a truncated recording): zero lengths, then 1
    // byte, too few for a length field. No NAL unit, so no size from it.
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    let data = movie(
        b"avc1",
        &config,
        &[nal_sample(&[SLICE.to_vec()]), vec![0u8; 517]],
    );
    assert_eq!(scanned(&data), Some((320, 240)));
    // A zero length before an SPS still finds the SPS.
    let mut sample = vec![0, 0, 0, 0];
    sample.extend(nal_sample(&[avc_sps(40, 30)]));
    let data = movie(b"avc1", &config, &[sample]);
    assert_eq!(scanned(&data), Some((640, 480)));
}

#[test]
fn samples_that_cannot_be_walked_give_no_bound() {
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    // A NAL length past the end of its sample.
    let mut broken = nal_sample(&[SLICE.to_vec()]);
    broken[3] = 200;
    let data = movie(b"avc1", &config, &[broken]);
    assert_eq!(scanned(&data), None);
    // An SPS in the record that cannot be read: a decoder applies it to
    // samples without one, so the in-band SPS is no bound.
    let unreadable = bx(b"avcC", &avcc(&[vec![0x67, 77, 0]]));
    let data = movie(
        b"avc1",
        &unreadable,
        &[nal_sample(&[avc_sps(20, 15), SLICE.to_vec()])],
    );
    assert_eq!(scanned(&data), None);
    // An SPS in a sample that cannot be read.
    let data = movie(
        b"avc1",
        &config,
        &[nal_sample(&[vec![0x67, 77, 0], SLICE.to_vec()])],
    );
    assert_eq!(scanned(&data), None);
}

fn vp9_key(width: u32, height: u32) -> Vec<u8> {
    let mut frame = pack(
        &[
            F::U(2, 2),
            F::U(0, 2),
            F::U(0, 1),
            F::U(0, 1),
            F::U(1, 1),
            F::U(0, 1),
            F::U(0x49_8342, 24),
            F::U(1, 3),
            F::U(0, 1),
            F::U(u64::from(width - 1), 16),
            F::U(u64::from(height - 1), 16),
        ],
        false,
    );
    frame.extend([0u8; 8]);
    frame
}

#[test]
fn vp9_frame_headers() {
    // vpcC carries no size; a later key frame is larger than the entry.
    let config = full(b"vpcC", 1, 0, &[0, 10, 0x80, 2, 2, 2, 0, 0]);
    let data = movie(b"vp09", &config, &[vp9_key(320, 240), vp9_key(3840, 2160)]);
    assert_eq!(probed(&data).tracks[0].description.parameter_set_size, None);
    assert_eq!(scanned(&data), Some((3840, 2160)));
    // A superframe of two key frames.
    let (a, b) = (vp9_key(320, 240), vp9_key(1280, 720));
    let marker = 0xc0 | 1;
    let mut superframe = cat(&[a.clone(), b.clone()]);
    superframe.extend([
        marker,
        u8::try_from(a.len()).unwrap(),
        u8::try_from(b.len()).unwrap(),
        marker,
    ]);
    let data = movie(b"vp09", &config, &[superframe]);
    assert_eq!(scanned(&data), Some((1280, 720)));
    // A frame without the VP9 frame marker.
    let data = movie(b"vp09", &config, &[vec![0u8; 16]]);
    assert_eq!(scanned(&data), None);
}

fn prores_frame(width: u16, height: u16) -> Vec<u8> {
    let mut frame = vec![0, 0, 0, 40, b'i', b'c', b'p', b'f', 0, 148, 0, 0];
    frame.extend(b"apl0");
    frame.extend(width.to_be_bytes());
    frame.extend(height.to_be_bytes());
    frame.extend([0u8; 20]);
    frame
}

#[test]
fn prores_frame_headers() {
    let data = movie(
        b"apch",
        &[],
        &[prores_frame(320, 240), prores_frame(4096, 2160)],
    );
    assert_eq!(scanned(&data), Some((4096, 2160)));
    let mut bad = prores_frame(320, 240);
    bad[4] = b'x';
    let data = movie(b"apch", &[], &[bad]);
    assert_eq!(scanned(&data), None);
}

fn av1_sequence_obu(width: u32, height: u32) -> Vec<u8> {
    let payload = pack(
        &[
            F::U(0, 3),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 1),
            F::U(0, 5),
            F::U(0, 12),
            F::U(8, 5),
            F::U(0, 1),
            F::U(15, 4),
            F::U(15, 4),
            F::U(u64::from(width - 1), 16),
            F::U(u64::from(height - 1), 16),
        ],
        true,
    );
    let mut obu = vec![(1 << 3) | 0x02, u8::try_from(payload.len()).unwrap()];
    obu.extend(payload);
    obu
}

#[test]
fn av1_sequence_headers() {
    let header = av1_sequence_obu(320, 240);
    let mut record = vec![0x81, 0x08, 0x0c, 0x00];
    record.extend(&header);
    let config = bx(b"av1C", &record);
    // A temporal delimiter, then a larger sequence header in the sample.
    let sample = cat(&[vec![(2 << 3) | 0x02, 0], av1_sequence_obu(7680, 4320)]);
    let data = movie(b"av01", &config, &[sample]);
    assert_eq!(
        probed(&data).tracks[0].description.parameter_set_size,
        Some((320, 240))
    );
    assert_eq!(scanned(&data), Some((7680, 4320)));
    // An OBU size past the end of the sample.
    let data = movie(b"av01", &config, &[vec![(6 << 3) | 0x02, 100, 0]]);
    assert_eq!(scanned(&data), None);
}

#[test]
fn the_scan_is_bounded_by_the_work_limit() {
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    // 2,000 tiny NAL units in one sample.
    let sample = nal_sample(&vec![SLICE.to_vec(); 2000]);
    let data = movie(b"avc1", &config, &[sample]);
    let movie = probed(&data);
    let limits = Limits {
        max_sample_work: 100,
        ..Limits::default()
    };
    let mut budget = Budget::new(&limits);
    assert!(
        movie
            .scan_frame_sizes(0, SliceSource::new(&data), &limits, &mut budget)
            .is_err()
    );
}

/// `data` (a one-track movie of [`movie`]) with copies of its track, track
/// IDs 2 to `tracks`, over the same samples.
fn with_track_copies(data: &[u8], tracks: u32) -> Vec<u8> {
    let mut data = data.to_vec();
    let at = |data: &[u8], kind: &[u8]| data.windows(4).position(|w| w == kind).unwrap() - 4;
    let (moov, trak) = (at(&data, b"moov"), at(&data, b"trak"));
    let trak_len = u32::from_be_bytes(data[trak..trak + 4].try_into().unwrap()) as usize;
    let original = data[trak..trak + trak_len].to_vec();
    for id in 2..=tracks {
        let mut copy = original.clone();
        // After the trak and tkhd headers, version and flags, and the two
        // times: track_ID.
        copy[28..32].copy_from_slice(&id.to_be_bytes());
        data.extend_from_slice(&copy);
    }
    let moov_len = u32::try_from(data.len() - moov).unwrap();
    data[moov..moov + 4].copy_from_slice(&moov_len.to_be_bytes());
    data
}

#[test]
fn one_work_budget_bounds_the_scan_of_every_track() {
    let one = movie(b"apch", &[], &vec![prores_frame(320, 240); 4]);
    let scan = |data: &[u8], max_sample_work| {
        let limits = Limits {
            max_sample_work,
            ..Limits::default()
        };
        probed(data)
            .scan_all_frame_sizes(SliceSource::new(data), &limits)
            .unwrap()
    };
    // The work one track's scan needs.
    let need = (1..64)
        .find(|&w| scan(&one, w) == [Some((320, 240))])
        .unwrap();
    // Two tracks share it: the second is unknown rather than scanned on a
    // budget of its own, so `max_sample_work` bounds the whole scan.
    let two = with_track_copies(&one, 2);
    assert_eq!(probed(&two).tracks.len(), 2);
    assert_eq!(scan(&two, need), [Some((320, 240)), None]);
    assert_eq!(scan(&two, 2 * need), [Some((320, 240)), Some((320, 240))]);
}

/// An H.264 Main profile SPS for 20 × 15 macroblocks whose video usability
/// information has a video signal type (ITU-T H.264 E.1.1): the range flag
/// and, with `colour`, a colour description.
fn avc_sps_signal(full_range: bool, colour: Option<(u8, u8, u8)>) -> Vec<u8> {
    let mut fields = vec![
        F::U(77, 8),
        F::U(0, 8),
        F::U(40, 8),
        F::Ue(0),
        F::Ue(0),
        F::Ue(2),
        F::Ue(1),
        F::U(0, 1),
        F::Ue(19),
        F::Ue(14),
        F::U(1, 1),
        F::U(1, 1),
        F::U(0, 1),
        // vui_parameters_present_flag, aspect ratio and overscan info absent,
        // video_signal_type_present_flag, video_format 5, the range flag.
        F::U(1, 1),
        F::U(0, 1),
        F::U(0, 1),
        F::U(1, 1),
        F::U(5, 3),
        F::U(u64::from(full_range), 1),
        F::U(u64::from(colour.is_some()), 1),
    ];
    if let Some((p, t, m)) = colour {
        fields.extend([F::U(p.into(), 8), F::U(t.into(), 8), F::U(m.into(), 8)]);
    }
    // No chroma location, timing, HRD, picture structure or restrictions.
    fields.extend([F::U(0, 1); 6]);
    let mut nal = vec![0x67];
    nal.extend(pack(&fields, true));
    nal
}

/// `colr` nclx with these code points and range.
fn colr(p: u16, t: u16, m: u16, full: bool) -> Vec<u8> {
    let mut payload = b"nclx".to_vec();
    for v in [p, t, m] {
        payload.extend(v.to_be_bytes());
    }
    payload.push(if full { 0x80 } else { 0 });
    bx(b"colr", &payload)
}

#[test]
fn parameter_set_colour_is_kept_raw_next_to_the_box() {
    use coreav_isobmff::Color;
    let color = |data: &[u8]| {
        let d = &probed(data).tracks[0].description;
        (d.color, d.parameter_set_color)
    };
    // A BT.709 box over an SPS that says SMPTE 170M, full range: each as
    // stored, neither changed by the other.
    let config = cat(&[
        bx(b"avcC", &avcc(&[avc_sps_signal(true, Some((6, 6, 6)))])),
        colr(1, 1, 1, false),
    ]);
    let (boxed, set) = color(&movie(b"avc1", &config, &[SLICE.to_vec()]));
    let c = |p, t, m, r| Color {
        primaries: p,
        transfer: t,
        matrix: m,
        full_range: r,
    };
    assert_eq!(boxed, c(Some(1), Some(1), Some(1), Some(false)));
    assert_eq!(set, Some(c(Some(6), Some(6), Some(6), Some(true))));
    // "Unspecified" (2) stays 2; a range without a colour description.
    let config = bx(b"avcC", &avcc(&[avc_sps_signal(false, Some((2, 2, 2)))]));
    let (boxed, set) = color(&movie(b"avc1", &config, &[SLICE.to_vec()]));
    assert_eq!(boxed, Color::default());
    assert_eq!(set, Some(c(Some(2), Some(2), Some(2), Some(false))));
    let config = bx(b"avcC", &avcc(&[avc_sps_signal(true, None)]));
    let (_, set) = color(&movie(b"avc1", &config, &[SLICE.to_vec()]));
    assert_eq!(set, Some(c(None, None, None, Some(true))));
    // No video usability information: nothing signalled.
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    let (_, set) = color(&movie(b"avc1", &config, &[SLICE.to_vec()]));
    assert_eq!(set, Some(Color::default()));
    // No parameter set in the record: no parameter-set colour at all.
    let (_, set) = color(&movie(b"avc1", &bx(b"avcC", &avcc(&[])), &[SLICE.to_vec()]));
    assert_eq!(set, None);
}

/// A source that records the regions it is advised of.
struct Advised<'a> {
    data: SliceSource<'a>,
    regions: Vec<Vec<(u64, u64)>>,
}

impl coreav_isobmff::Source for Advised<'_> {
    fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), coreav_isobmff::Error> {
        self.data.read_exact_at(offset, buf)
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        self.regions.push(regions.to_vec());
    }

    fn wants_regions(&mut self) -> bool {
        true
    }
}

#[test]
fn a_header_only_scan_advises_the_regions_it_reads() {
    // ProRes: 4 KiB from the start of each sample (the window a header
    // read fills), in increasing order, before any frame is read.
    let mut large = prores_frame(4096, 2160);
    large.resize(10_000, 0);
    let frames = [prores_frame(320, 240), large, prores_frame(640, 480)];
    let data = movie(b"apch", &[], &frames);
    let probed_movie = probed(&data);
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let mut advised = Advised {
        data: SliceSource::new(&data),
        regions: Vec::new(),
    };
    let size = probed_movie
        .scan_frame_sizes(0, &mut advised, &limits, &mut budget)
        .unwrap();
    assert_eq!(size, Some((4096, 2160)));
    let packets: Vec<_> = probed_movie
        .packets(
            0,
            SliceSource::new(&data),
            &limits,
            &mut Budget::new(&limits),
        )
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let expected: Vec<(u64, u64)> = packets.iter().map(|p| (p.offset, 4096)).collect();
    // The advice, then its end once the track is scanned (an empty list).
    assert_eq!(advised.regions, vec![expected, Vec::new()]);
    // H.264: the scan walks each sample's NAL units, so it advises nothing.
    let config = bx(b"avcC", &avcc(&[avc_sps(20, 15)]));
    let data = movie(b"avc1", &config, &[nal_sample(&[SLICE.to_vec()])]);
    let movie = probed(&data);
    let mut advised = Advised {
        data: SliceSource::new(&data),
        regions: Vec::new(),
    };
    movie
        .scan_frame_sizes(0, &mut advised, &limits, &mut budget)
        .unwrap();
    assert!(advised.regions.is_empty());
}

#[test]
fn advice_is_listed_only_for_a_source_that_wants_it_and_only_for_a_bounded_track() {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    // A source that cannot use the advice is not given any: the listing
    // is not even built (the native command reads files).
    struct Plain<'a>(SliceSource<'a>, usize);
    impl coreav_isobmff::Source for Plain<'_> {
        fn len(&mut self) -> Result<u64, coreav_isobmff::Error> {
            self.0.len()
        }
        fn read_exact_at(
            &mut self,
            offset: u64,
            buf: &mut [u8],
        ) -> Result<(), coreav_isobmff::Error> {
            self.0.read_exact_at(offset, buf)
        }
        fn will_read_regions(&mut self, _: &[(u64, u64)]) {
            self.1 += 1;
        }
    }
    let data = movie(b"apch", &[], &[prores_frame(320, 240)]);
    let mut plain = Plain(SliceSource::new(&data), 0);
    probed(&data)
        .scan_frame_sizes(0, &mut plain, &limits, &mut budget)
        .unwrap();
    assert_eq!(plain.1, 0);
    // More samples than the advice holds (65,536): no advice, the same
    // result.
    let frames = vec![prores_frame(320, 240); (1 << 16) + 1];
    let data = movie(b"apch", &[], &frames);
    let mut advised = Advised {
        data: SliceSource::new(&data),
        regions: Vec::new(),
    };
    let size = probed(&data)
        .scan_frame_sizes(0, &mut advised, &limits, &mut Budget::new(&limits))
        .unwrap();
    assert_eq!(size, Some((320, 240)));
    assert!(advised.regions.is_empty());
}

#[test]
fn advice_over_all_tracks_shares_one_bound() {
    // The bound of 65,536 advised samples holds for every track scanned
    // under one budget together: a file of many tracks cannot list (or
    // advise) more. 40,000 samples fit once, not twice; the sizes are the
    // same either way.
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let frames = vec![prores_frame(320, 240); 40_000];
    let data = movie(b"apch", &[], &frames);
    let probed_movie = probed(&data);
    for round in 0..2 {
        let mut advised = Advised {
            data: SliceSource::new(&data),
            regions: Vec::new(),
        };
        let size = probed_movie
            .scan_frame_sizes(0, &mut advised, &limits, &mut budget)
            .unwrap();
        assert_eq!(size, Some((320, 240)));
        let lengths: Vec<usize> = advised.regions.iter().map(Vec::len).collect();
        assert_eq!(
            lengths,
            if round == 0 { vec![40_000, 0] } else { vec![] },
            "round {round}"
        );
    }
}
