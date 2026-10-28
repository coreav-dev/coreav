//! A movie builder for text tracks (ISO/IEC 14496-12 boxes, 3GPP TS 26.245
//! and QuickTime text sample entries), shared with `coreav-text`'s tests.
#![allow(dead_code)]

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

fn u32s(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// One track of [`movie`].
#[derive(Clone)]
pub struct TrackSpec {
    pub id: u32,
    pub handler: [u8; 4],
    /// The whole sample entry box.
    pub entry: Vec<u8>,
    /// Sample bytes and decode durations, in order.
    pub samples: Vec<(Vec<u8>, u32)>,
    pub timescale: u32,
    pub enabled: bool,
    /// `track_ID`s of `chap` track references.
    pub chap: Vec<u32>,
    /// Edit list entries: segment duration in movie ticks and media time.
    pub elst: Option<Vec<(u32, i32)>>,
    /// Composition offsets, one per sample.
    pub ctts: Option<Vec<u32>>,
    /// `tkhd` and `tref` after `mdia` instead of before it.
    pub tkhd_last: bool,
    /// The track header's width and height, 16.16 fixed point.
    pub size: (u32, u32),
}

impl TrackSpec {
    /// A 3GPP timed text track with these samples.
    pub fn tx3g(id: u32, samples: Vec<(Vec<u8>, u32)>) -> Self {
        Self {
            id,
            handler: *b"sbtl",
            entry: tx3g_entry(),
            samples,
            timescale: 1000,
            enabled: true,
            chap: Vec::new(),
            elst: None,
            ctts: None,
            tkhd_last: false,
            size: (0, 0),
        }
    }

    /// A QuickTime text track (a chapter track, say), disabled.
    pub fn qt_text(id: u32, samples: Vec<(Vec<u8>, u32)>) -> Self {
        Self {
            handler: *b"text",
            entry: qt_text_entry(),
            enabled: false,
            ..Self::tx3g(id, samples)
        }
    }

    /// A video track with one 100-byte sample per duration, enabled.
    pub fn video(id: u32, durations: &[u32]) -> Self {
        Self {
            handler: *b"vide",
            entry: avc_entry(),
            samples: durations.iter().map(|d| (vec![0xAB; 100], *d)).collect(),
            ..Self::tx3g(id, Vec::new())
        }
    }
}

/// A `tx3g` sample entry (TS 26.245 5.16) with a one-font table.
pub fn tx3g_entry() -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&0u32.to_be_bytes()); // displayFlags
    body.extend_from_slice(&[1, 0xFF]); // justification
    body.extend_from_slice(&[0, 0, 0, 0xFF]); // background
    body.extend_from_slice(&[0; 8]); // default text box
    body.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 18, 0xFF, 0xFF, 0xFF, 0xFF]); // style
    let mut ftab = 1u16.to_be_bytes().to_vec();
    ftab.extend_from_slice(&[0, 1, 5]);
    ftab.extend_from_slice(b"Serif");
    body.extend_from_slice(&bx(b"ftab", &ftab));
    bx(b"tx3g", &body)
}

/// A QuickTime `text` sample entry (QuickTime File Format, text sample
/// description) with an empty font name.
pub fn qt_text_entry() -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0; 4]); // display flags
    body.extend_from_slice(&[0; 4]); // justification
    body.extend_from_slice(&[0; 6]); // background colour
    body.extend_from_slice(&[0; 8]); // text box
    body.extend_from_slice(&[0; 8]); // reserved
    body.extend_from_slice(&[0; 2]); // font number
    body.extend_from_slice(&[0; 2]); // font face
    body.extend_from_slice(&[0; 3]); // reserved
    body.extend_from_slice(&[0; 6]); // foreground colour
    body.push(0); // text name
    bx(b"text", &body)
}

pub fn avc_entry() -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 16]);
    body.extend_from_slice(&320u16.to_be_bytes());
    body.extend_from_slice(&240u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 50]);
    body.extend_from_slice(&bx(b"avcC", &[1, 100, 0, 30, 0xff, 0xe0, 0]));
    bx(b"avc1", &body)
}

/// A text sample: the 16-bit length, the text, then `boxes`.
pub fn text_sample(text: &[u8], boxes: &[Vec<u8>]) -> Vec<u8> {
    let mut out = (text.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(text);
    for b in boxes {
        out.extend_from_slice(b);
    }
    out
}

/// A `styl` box with one style record over `[start, end)`.
pub fn styl(start: u16, end: u16) -> Vec<u8> {
    let mut body = 1u16.to_be_bytes().to_vec();
    body.extend_from_slice(&start.to_be_bytes());
    body.extend_from_slice(&end.to_be_bytes());
    body.extend_from_slice(&[0, 1, 1, 18, 0xFF, 0, 0, 0xFF]);
    bx(b"styl", &body)
}

/// The movie: `ftyp`, `mdat` with every track's samples in one chunk per
/// track, then `moov` (movie timescale 1000).
pub fn movie(tracks: &[TrackSpec]) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\x02\0isom");
    let mut data = Vec::new();
    let mut chunk_offsets = Vec::new();
    let base = ftyp.len() + 8;
    for track in tracks {
        chunk_offsets.push((base + data.len()) as u32);
        for (bytes, _) in &track.samples {
            data.extend_from_slice(bytes);
        }
    }
    let mdat = bx(b"mdat", &data);
    let longest = tracks
        .iter()
        .map(|t| {
            t.samples.iter().map(|s| u64::from(s.1)).sum::<u64>() * 1000 / u64::from(t.timescale)
        })
        .max()
        .unwrap_or(0) as u32;
    let mut mvhd = u32s(&[0, 0, 1000, longest]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut moov = full(b"mvhd", 0, 0, &mvhd);
    for (track, offset) in tracks.iter().zip(chunk_offsets) {
        moov.extend_from_slice(&trak(track, offset));
    }
    [ftyp, mdat, bx(b"moov", &moov)].concat()
}

fn trak(t: &TrackSpec, chunk_offset: u32) -> Vec<u8> {
    let count = t.samples.len() as u32;
    let media_duration: u32 = t.samples.iter().map(|s| s.1).sum();
    let mut tkhd = u32s(&[0, 0, t.id, 0, media_duration * 1000 / t.timescale]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    tkhd.extend_from_slice(&u32s(&[t.size.0, t.size.1]));
    let tkhd = full(b"tkhd", 0, if t.enabled { 3 } else { 2 }, &tkhd);
    let mut mdhd = u32s(&[0, 0, t.timescale, media_duration]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(&t.handler);
    hdlr.extend_from_slice(&[0u8; 13]);
    let mut stsd = u32s(&[1]);
    stsd.extend_from_slice(&t.entry);
    let mut stts = u32s(&[count]);
    let mut stsz = u32s(&[0, count]);
    for (bytes, duration) in &t.samples {
        stts.extend_from_slice(&u32s(&[1, *duration]));
        stsz.extend_from_slice(&u32s(&[bytes.len() as u32]));
    }
    let mut stbl = [
        full(b"stsd", 0, 0, &stsd),
        full(b"stts", 0, 0, &stts),
        full(b"stsc", 0, 0, &u32s(&[1, 1, count.max(1), 1])),
        full(b"stsz", 0, 0, &stsz),
        full(b"stco", 0, 0, &u32s(&[1, chunk_offset])),
    ]
    .concat();
    if let Some(offsets) = &t.ctts {
        let mut ctts = u32s(&[offsets.len() as u32]);
        for o in offsets {
            ctts.extend_from_slice(&u32s(&[1, *o]));
        }
        stbl.extend_from_slice(&full(b"ctts", 0, 0, &ctts));
    }
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(b"minf", &[bx(b"dinf", &dref), bx(b"stbl", &stbl)].concat());
    let mdia = bx(
        b"mdia",
        &[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf].concat(),
    );
    let mut parts = Vec::new();
    if !t.tkhd_last {
        parts.push(tkhd.clone());
    }
    let tref = (!t.chap.is_empty()).then(|| bx(b"tref", &bx(b"chap", &u32s(&t.chap))));
    if !t.tkhd_last {
        parts.extend(tref.clone());
    }
    if let Some(edits) = &t.elst {
        let mut elst = u32s(&[edits.len() as u32]);
        for (segment, media_time) in edits {
            elst.extend_from_slice(&u32s(&[*segment, *media_time as u32, 0x10000]));
        }
        parts.push(bx(b"edts", &full(b"elst", 0, 0, &elst)));
    }
    parts.push(mdia);
    if t.tkhd_last {
        parts.extend(tref);
        parts.push(tkhd);
    }
    bx(b"trak", &parts.concat())
}
