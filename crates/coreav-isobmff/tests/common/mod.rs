//! Builders for small ISO base media files, written from ISO/IEC 14496-12.
#![allow(dead_code)]

pub mod fragments;
pub mod sound;

pub fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

pub fn full(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut body = vec![version];
    body.extend_from_slice(&flags.to_be_bytes()[1..]);
    body.extend_from_slice(payload);
    bx(kind, &body)
}

pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

/// What follows a version 0 `mvhd`'s duration: rate and volume (zero), the
/// identity matrix as real files carry it, and zero pre-defined fields and
/// next track ID.
fn mvhd_rest() -> Vec<u8> {
    let mut rest = vec![0u8; 16];
    rest.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    rest.extend_from_slice(&[0u8; 28]);
    rest
}

pub fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

pub const MOVIE_TIMESCALE: u32 = 1000;
pub const TIMESCALE: u32 = 12_800;

/// Parameters of a one-track H.264 movie with `sizes.len()` samples of
/// 512 ticks in one chunk.
pub struct Spec {
    pub sizes: Vec<u32>,
    pub composition: Vec<i32>,
    pub sync: Vec<u32>,
    pub elst: Option<Vec<(u32, i32)>>,
    pub sample_entry: [u8; 4],
    pub stsz_count: Option<u32>,
    /// `stts` entries (count, delta) instead of one entry of 512 ticks.
    pub stts: Option<Vec<(u32, u32)>>,
    /// `ctts` entries (count, offset) instead of one per sample of
    /// `composition`; an empty list leaves `ctts` out.
    pub ctts: Option<Vec<(u32, i32)>>,
    /// Samples per chunk, the chunks back to back in `mdat`, instead of one
    /// chunk.
    pub chunks: Option<Vec<u32>>,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            sizes: vec![100, 50, 60, 70],
            composition: vec![1024, 2048, 0, 512],
            sync: vec![1],
            elst: Some(vec![(80, 1024)]),
            sample_entry: *b"avc1",
            stsz_count: None,
            stts: None,
            ctts: None,
            chunks: None,
        }
    }
}

pub fn visual_entry(kind: &[u8; 4], extra: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 16]);
    body.extend_from_slice(&320u16.to_be_bytes());
    body.extend_from_slice(&240u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 50]);
    body.extend_from_slice(extra);
    bx(kind, &body)
}

/// Builds the movie with `mdat` first; returns the file bytes.
pub fn movie(spec: &Spec) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let data: Vec<u8> = spec
        .sizes
        .iter()
        .flat_map(|s| vec![0xAB; *s as usize])
        .collect();
    let mdat = bx(b"mdat", &data);
    let chunk_offset = (ftyp.len() + 8) as u32;
    let count = spec.sizes.len() as u32;
    let duration = match &spec.stts {
        Some(entries) => entries
            .iter()
            .map(|(n, delta)| n.saturating_mul(*delta))
            .fold(0u32, u32::saturating_add),
        None => 512 * count,
    };
    let mut mvhd = u32s(&[
        0,
        0,
        MOVIE_TIMESCALE,
        duration * MOVIE_TIMESCALE / TIMESCALE,
    ]);
    mvhd.extend_from_slice(&mvhd_rest());
    let matrix = u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]);
    let mut tkhd = u32s(&[0, 0, 1, 0, duration]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&matrix);
    tkhd.extend_from_slice(&u32s(&[320 << 16, 240 << 16]));
    let mut mdhd = u32s(&[0, 0, TIMESCALE, duration]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.extend_from_slice(b"VideoHandler\0");
    let entry = visual_entry(
        &spec.sample_entry,
        &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]),
    );
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
    let ctts: Vec<(u32, i32)> = match &spec.ctts {
        Some(entries) => entries.clone(),
        None => spec.composition.iter().map(|c| (1, *c)).collect(),
    };
    let stts = match &spec.stts {
        Some(entries) => entries.clone(),
        None => vec![(count, 512)],
    };
    let chunks = spec.chunks.clone().unwrap_or_else(|| vec![count]);
    // One `stsc` entry wherever the samples per chunk change.
    let mut stsc = Vec::new();
    let mut offsets = Vec::new();
    let (mut at, mut sample, mut previous) = (chunk_offset, 0usize, None);
    for (i, n) in chunks.iter().enumerate() {
        if previous != Some(*n) {
            stsc.push([i as u32 + 1, *n, 1]);
        }
        previous = Some(*n);
        offsets.push(at);
        at += spec
            .sizes
            .iter()
            .skip(sample)
            .take(*n as usize)
            .sum::<u32>();
        sample += *n as usize;
    }
    let mut stsz = u32s(&[0, spec.stsz_count.unwrap_or(count)]);
    stsz.extend_from_slice(&u32s(&spec.sizes));
    let mut stbl_parts = vec![
        full(b"stsd", 0, 0, &stsd),
        full(
            b"stts",
            0,
            0,
            &[
                u32s(&[stts.len() as u32]),
                stts.iter().flat_map(|(n, d)| u32s(&[*n, *d])).collect(),
            ]
            .concat(),
        ),
        full(
            b"stsc",
            0,
            0,
            &[u32s(&[stsc.len() as u32]), u32s(&stsc.concat())].concat(),
        ),
        full(b"stsz", 0, 0, &stsz),
        full(
            b"stco",
            0,
            0,
            &[u32s(&[offsets.len() as u32]), u32s(&offsets)].concat(),
        ),
    ];
    if spec.ctts.as_ref().is_none_or(|entries| !entries.is_empty()) {
        stbl_parts.insert(
            2,
            full(
                b"ctts",
                1,
                0,
                &[
                    u32s(&[ctts.len() as u32]),
                    ctts.iter()
                        .flat_map(|(n, c)| u32s(&[*n, *c as u32]))
                        .collect(),
                ]
                .concat(),
            ),
        );
    }
    if !spec.sync.is_empty() {
        let mut stss = u32s(&[spec.sync.len() as u32]);
        stss.extend_from_slice(&u32s(&spec.sync));
        stbl_parts.push(full(b"stss", 0, 0, &stss));
    }
    let stbl = bx(b"stbl", &cat(&stbl_parts));
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
    let mut trak_parts = vec![full(b"tkhd", 0, 3, &tkhd)];
    if let Some(edits) = &spec.elst {
        let mut elst = u32s(&[edits.len() as u32]);
        for (segment, media_time) in edits {
            elst.extend_from_slice(&u32s(&[*segment, *media_time as u32, 0x10000]));
        }
        trak_parts.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    trak_parts.push(mdia);
    let moov = bx(
        b"moov",
        &cat(&[full(b"mvhd", 0, 0, &mvhd), bx(b"trak", &cat(&trak_parts))]),
    );
    cat(&[ftyp, mdat, moov])
}

/// A fragmented movie (empty sample tables, `mdhd` duration 0) with an
/// optional `mehd` and an optional single media edit, followed by one
/// fragment of four 100-byte samples of 512 ticks.
pub fn fragmented_movie(mehd: Option<u32>, edit: Option<(u32, i32)>) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 0]);
    mvhd.extend_from_slice(&mvhd_rest());
    let mut tkhd = u32s(&[0, 0, 1, 0, 0]);
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
    let mut mdhd = u32s(&[0, 0, TIMESCALE, 0]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"vide");
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&visual_entry(
        b"avc1",
        &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]),
    ));
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[0])),
            full(b"stsc", 0, 0, &u32s(&[0])),
            full(b"stsz", 0, 0, &u32s(&[0, 0])),
            full(b"stco", 0, 0, &u32s(&[0])),
        ]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[
            full(b"mdhd", 0, 0, &mdhd),
            full(b"hdlr", 0, 0, &hdlr),
            bx(b"minf", &stbl),
        ]),
    );
    let mut trak_parts = vec![full(b"tkhd", 0, 3, &tkhd)];
    if let Some((segment, media_time)) = edit {
        let elst = u32s(&[1, segment, media_time as u32, 0x10000]);
        trak_parts.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    trak_parts.push(mdia);
    let mut mvex_parts = vec![full(b"trex", 0, 0, &u32s(&[1, 1, 512, 100, 0]))];
    if let Some(duration) = mehd {
        mvex_parts.push(full(b"mehd", 0, 0, &u32s(&[duration])));
    }
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            bx(b"trak", &cat(&trak_parts)),
            bx(b"mvex", &cat(&mvex_parts)),
        ]),
    );
    let tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
    let tfdt = full(b"tfdt", 0, 0, &u32s(&[0]));
    // trun: data offset only; sizes and durations come from trex.
    let traf_len = 8 + tfhd.len() + tfdt.len() + 20;
    let moof_len = 8 + 16 + traf_len;
    let trun = full(b"trun", 0, 0x1, &u32s(&[4, (moof_len + 8) as u32]));
    let moof = bx(
        b"moof",
        &cat(&[
            full(b"mfhd", 0, 0, &u32s(&[1])),
            bx(b"traf", &cat(&[tfhd, tfdt, trun])),
        ]),
    );
    assert_eq!(moof.len(), moof_len);
    let mdat = bx(b"mdat", &[0xAB; 400]);
    cat(&[ftyp, moov, moof, mdat])
}

/// A fragmented movie of two tracks of four 100-byte samples of 512 ticks
/// each, the second with sample entry `entry`.
///
/// With `external`, the second track keeps its media in another file: its
/// `dref` entry has the self-contained flag clear (ISO/IEC 14496-12 8.7.2)
/// and its track fragment names a `base_data_offset` far outside this file,
/// as an offset into that one. Otherwise both tracks' samples lie in the
/// `mdat` here, the first track's first.
pub fn two_track_fragmented_movie(entry: &[u8; 4], external: bool) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 0]);
    mvhd.extend_from_slice(&mvhd_rest());
    let trak = |track_id: u32, entry: &[u8; 4], self_contained: bool| {
        let mut tkhd = u32s(&[0, 0, track_id, 0, 0]);
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
        let mut mdhd = u32s(&[0, 0, TIMESCALE, 0]);
        mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
        let mut hdlr = u32s(&[0]);
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0u8; 13]);
        let mut stsd = u32s(&[1]);
        stsd.extend_from_slice(&visual_entry(
            entry,
            &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]),
        ));
        let stbl = bx(
            b"stbl",
            &cat(&[
                full(b"stsd", 0, 0, &stsd),
                full(b"stts", 0, 0, &u32s(&[0])),
                full(b"stsc", 0, 0, &u32s(&[0])),
                full(b"stsz", 0, 0, &u32s(&[0, 0])),
                full(b"stco", 0, 0, &u32s(&[0])),
            ]),
        );
        // `url ` with flag 1 is this file; with no flags, another one.
        let dref = full(
            b"dref",
            0,
            0,
            &cat(&[u32s(&[1]), full(b"url ", 0, u32::from(self_contained), &[])]),
        );
        let mdia = bx(
            b"mdia",
            &cat(&[
                full(b"mdhd", 0, 0, &mdhd),
                full(b"hdlr", 0, 0, &hdlr),
                bx(b"minf", &cat(&[bx(b"dinf", &dref), stbl])),
            ]),
        );
        bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
    };
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            trak(1, b"avc1", true),
            trak(2, entry, !external),
            bx(
                b"mvex",
                &cat(&[
                    full(b"trex", 0, 0, &u32s(&[1, 1, 512, 100, 0])),
                    full(b"trex", 0, 0, &u32s(&[2, 1, 512, 100, 0])),
                ]),
            ),
        ]),
    );
    // The first track's fragment takes a data offset from the `moof`, as in
    // `fragmented_movie`. The second takes one too, past the first track's
    // 400 bytes, unless its media is elsewhere: then it names a
    // `base_data_offset` into the file it references, which this file's
    // length says nothing about.
    let first_tfhd = full(b"tfhd", 0, 0x02_0000, &u32s(&[1]));
    let second_tfhd = if external {
        let mut body = u32s(&[2]);
        body.extend_from_slice(&1_000_000u64.to_be_bytes());
        full(b"tfhd", 0, 0x1, &body)
    } else {
        full(b"tfhd", 0, 0x02_0000, &u32s(&[2]))
    };
    let tfdt = full(b"tfdt", 0, 0, &u32s(&[0]));
    let first_traf_len = 8 + first_tfhd.len() + tfdt.len() + 20;
    let second_traf_len = 8 + second_tfhd.len() + tfdt.len() + 20;
    let moof_len = 8 + 16 + first_traf_len + second_traf_len;
    let first_trun = full(b"trun", 0, 0x1, &u32s(&[4, (moof_len + 8) as u32]));
    let second_offset = if external {
        0
    } else {
        (moof_len + 8 + 400) as u32
    };
    let second_trun = full(b"trun", 0, 0x1, &u32s(&[4, second_offset]));
    let moof = bx(
        b"moof",
        &cat(&[
            full(b"mfhd", 0, 0, &u32s(&[1])),
            bx(b"traf", &cat(&[first_tfhd, tfdt.clone(), first_trun])),
            bx(b"traf", &cat(&[second_tfhd, tfdt, second_trun])),
        ]),
    );
    assert_eq!(moof.len(), moof_len);
    cat(&[ftyp, moov, moof, bx(b"mdat", &[0xAB; 800])])
}

/// [`two_track_fragmented_movie`] whose second track keeps its media in
/// another file.
pub fn external_fragmented_movie() -> Vec<u8> {
    two_track_fragmented_movie(b"avc1", true)
}

/// One track of `av_movie`: handler, sample entry, and four 100-byte samples
/// of 512 ticks at `chunk_offset`.
fn simple_trak(
    track_id: u32,
    handler: &[u8; 4],
    entry: Vec<u8>,
    chunk_offset: u32,
    minf_first: bool,
) -> Vec<u8> {
    let mut tkhd = u32s(&[0, 0, track_id, 0, 2048]);
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
    let mut mdhd = u32s(&[0, 0, TIMESCALE, 2048]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(handler);
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&entry);
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &stsd),
            full(b"stts", 0, 0, &u32s(&[1, 4, 512])),
            full(b"stsc", 0, 0, &u32s(&[1, 1, 4, 1])),
            full(b"stsz", 0, 0, &u32s(&[100, 4])),
            full(b"stco", 0, 0, &u32s(&[1, chunk_offset])),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(b"minf", &cat(&[bx(b"dinf", &dref), stbl]));
    let (mdhd, hdlr) = (full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr));
    // ISO/IEC 14496-12 does not fix the order of the children of `mdia`.
    let mdia = if minf_first {
        bx(b"mdia", &cat(&[mdhd, minf, hdlr]))
    } else {
        bx(b"mdia", &cat(&[mdhd, hdlr, minf]))
    };
    bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
}

/// A sound sample entry (ISO/IEC 14496-12 12.2.3), version 0.
pub fn audio_entry(kind: &[u8; 4]) -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 8]);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 4]);
    body.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    bx(kind, &body)
}

/// A two-track movie: H.264 video (track 1) and audio with sample entry
/// `audio` (track 2), each with four 100-byte samples in one chunk.
pub fn av_movie(audio: &[u8; 4]) -> Vec<u8> {
    av_movie_with(audio_entry(audio), false)
}

/// An AAC sound sample entry: `mp4a` with an `esds` whose decoder
/// configuration has object type 0x40 (ISO/IEC 14496-1 7.2.6.6).
pub fn aac_entry() -> Vec<u8> {
    let esds = full(
        b"esds",
        0,
        0,
        &[
            3, 18, 0, 0, 0, 4, 13, 0x40, 0x15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ],
    );
    let mut body = audio_entry(b"mp4a").split_off(8);
    body.extend_from_slice(&esds);
    bx(b"mp4a", &body)
}

/// `av_movie` with a given audio sample entry and, optionally, `minf`
/// placed before `hdlr` in every `mdia`.
pub fn av_movie_with(audio: Vec<u8>, minf_first: bool) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    // Four 100-byte video samples, each one IDR slice NAL unit (4-byte
    // length; first_mb_in_slice 0, slice_type 7): the track has no sync
    // sample table, so a stream copy reads them to cut
    // (docs/design/cut-points.md). Then the audio.
    let mut data = Vec::new();
    for _ in 0..4 {
        data.extend_from_slice(&[0, 0, 0, 96, 0x65, 0x88]);
        data.extend_from_slice(&[0xAB; 94]);
    }
    data.extend_from_slice(&[0xAB; 400]);
    let mdat = bx(b"mdat", &data);
    let video_at = (ftyp.len() + 8) as u32;
    let mut mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 160]);
    mvhd.extend_from_slice(&mvhd_rest());
    let video = visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            simple_trak(1, b"vide", video, video_at, minf_first),
            simple_trak(2, b"soun", audio, video_at + 400, minf_first),
        ]),
    );
    cat(&[ftyp, mdat, moov])
}

/// A fragmented two-track movie with one `moof`: track 1 has four 100-byte
/// samples whose `trun` points into the `mdat`; track 2 has three 50-byte
/// samples, and its `traf` sets no base flags and its `trun` no data offset,
/// so its data starts where track 1's data ends (ISO/IEC 14496-12 8.8.7).
/// Sizes come from `trex` defaults, so computing that end needs track 1's
/// defaults. Returns the file and the offset of the `mdat` payload.
pub fn implicit_base_fragments() -> (Vec<u8>, u64) {
    let ftyp = bx(b"ftyp", b"iso6\0\0\x02\0");
    let mut mvhd = u32s(&[0, 0, MOVIE_TIMESCALE, 0]);
    mvhd.extend_from_slice(&mvhd_rest());
    let empty_trak = |id: u32, handler: &[u8; 4], entry: Vec<u8>| {
        let mut tkhd = u32s(&[0, 0, id, 0, 0]);
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
        let mut mdhd = u32s(&[0, 0, TIMESCALE, 0]);
        mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
        let mut hdlr = u32s(&[0]);
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0u8; 13]);
        let mut stsd = u32s(&[1]);
        stsd.extend_from_slice(&entry);
        let stbl = bx(
            b"stbl",
            &cat(&[
                full(b"stsd", 0, 0, &stsd),
                full(b"stts", 0, 0, &u32s(&[0])),
                full(b"stsc", 0, 0, &u32s(&[0])),
                full(b"stsz", 0, 0, &u32s(&[0, 0])),
                full(b"stco", 0, 0, &u32s(&[0])),
            ]),
        );
        let mdia = bx(
            b"mdia",
            &cat(&[
                full(b"mdhd", 0, 0, &mdhd),
                full(b"hdlr", 0, 0, &hdlr),
                bx(b"minf", &stbl),
            ]),
        );
        bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]))
    };
    let video = visual_entry(b"avc1", &bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
    let mvex = bx(
        b"mvex",
        &cat(&[
            full(b"trex", 0, 0, &u32s(&[1, 1, 512, 100, 0])),
            full(b"trex", 0, 0, &u32s(&[2, 1, 1024, 50, 0])),
        ]),
    );
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &mvhd),
            empty_trak(1, b"vide", video),
            empty_trak(2, b"soun", audio_entry(b"sowt")),
            mvex,
        ]),
    );
    // traf 1: no base flags (first traf: base = moof start), trun with a data
    // offset into the mdat. traf 2: no base flags, trun without data offset.
    let traf =
        |id: u32, trun: Vec<u8>| bx(b"traf", &cat(&[full(b"tfhd", 0, 0, &u32s(&[id])), trun]));
    let moof_len = (8 + 16 + (8 + 16 + 20) + (8 + 16 + 16)) as u32;
    let traf1 = traf(1, full(b"trun", 0, 0x1, &u32s(&[4, moof_len + 8])));
    let traf2 = traf(2, full(b"trun", 0, 0, &u32s(&[3])));
    let moof = bx(
        b"moof",
        &cat(&[full(b"mfhd", 0, 0, &u32s(&[1])), traf1, traf2]),
    );
    assert_eq!(moof.len() as u32, moof_len);
    let mdat = bx(b"mdat", &[0xAB; 550]);
    let mdat_payload = (ftyp.len() + moov.len() + moof.len() + 8) as u64;
    (cat(&[ftyp, moov, moof, mdat]), mdat_payload)
}
