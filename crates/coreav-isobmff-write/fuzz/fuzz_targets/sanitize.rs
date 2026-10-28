//! Arbitrary input through sanitization (`docs/design/sanitize.md`): it
//! either fails with an error or writes an output that the parser accepts,
//! with the movie box first and no metadata in its tree but what `Keep`
//! writes again (`udta` boxes holding only `kind` boxes and one `meta` box
//! with an `iTunSMPB` item), whose streams hold every sample of the kept source streams:
//! the same bytes, decode durations and composition offsets, in order.
#![no_main]

use coreav_isobmff::{
    Allowlist, Budget, BufferedSource, Limits, Packet, SliceSource, Source, probe,
};
use coreav_isobmff_write::{Keep, Metadata, OnUnsupported, RemuxOptions, remux_mp4};
use libfuzzer_sys::fuzz_target;

fn packets(data: &[u8], limits: &Limits, stream_index: Option<usize>, nth: usize) -> Vec<Packet> {
    let movie = probe(SliceSource::new(data), limits, &Allowlist::default()).expect("probe");
    let track = match stream_index {
        Some(index) => movie.tracks.iter().find(|t| t.stream_index == index),
        None => movie.tracks.get(nth),
    }
    .expect("the track");
    let mut budget = Budget::new(limits);
    movie
        .packets(track.index, SliceSource::new(data), limits, &mut budget)
        .expect("packets")
        .map(|p| p.expect("every packet is readable"))
        .collect()
}

fn bytes(data: &[u8], packet: &Packet) -> Vec<u8> {
    let mut buf = vec![0; packet.size as usize];
    SliceSource::new(data)
        .read_exact_at(packet.offset, &mut buf)
        .expect("packet bytes");
    buf
}

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_sample_work: 1 << 20,
        max_table_bytes: 1 << 20,
        ..Limits::default()
    };
    // The first byte picks the options: drop or fail on unsupported
    // streams, every stream or only the stream it names, and what to keep.
    let Some((&choice, input)) = data.split_first() else {
        return;
    };
    let options = RemuxOptions {
        streams: (choice & 2 != 0).then(|| vec![usize::from(choice >> 2)]),
        on_unsupported: if choice & 1 == 0 {
            OnUnsupported::Drop
        } else {
            OnUnsupported::Fail
        },
        metadata: Metadata::Sanitize(if choice & 0x80 == 0 {
            Keep::default()
        } else {
            Keep::NOTHING
        }),
    };
    let mut out = Vec::new();
    let source = BufferedSource::new(SliceSource::new(input));
    let Ok(summary) = remux_mp4(source, &limits, &Allowlist::default(), &options, &mut out) else {
        return;
    };
    assert_eq!(summary.bytes_written, out.len() as u64);
    assert_eq!(&out[4..8], b"ftyp");
    let movie = probe(
        SliceSource::new(&out),
        &Limits::default(),
        &Allowlist::default(),
    )
    .expect("the output parses");
    assert!(!movie.fragmented);
    assert_eq!(movie.tracks.len(), summary.tracks.len());
    let keep = match options.metadata {
        Metadata::Sanitize(keep) => keep,
        Metadata::Copy => unreachable!(),
    };
    let tree = box_tree(&out);
    assert!(
        !tree
            .iter()
            .any(|k| matches!(k, b"meta" | b"keys" | b"ilst"))
    );
    for (in_track, udta) in udta_children(&out) {
        for (kind, payload) in udta {
            match &kind {
                b"kind" => {
                    assert!(keep.kinds && in_track);
                    assert!(KINDS.iter().any(|(scheme, values)| values.iter().any(|v| {
                        payload == [&[0; 4], scheme.as_bytes(), b"\0", v.as_bytes(), b"\0"].concat()
                    })));
                }
                b"meta" => {
                    assert!(keep.gapless);
                    // Exactly the canonical item: only the three numbers vary.
                    let value = &payload[payload.len() - 116..];
                    assert!(canonical_gapless(value), "{value:?}");
                    assert_eq!(payload, gapless_meta(value));
                }
                other => panic!("{other:?} in a sanitized udta"),
            }
        }
    }
    for (nth, kept) in summary.tracks.iter().enumerate() {
        let written = packets(&out, &Limits::default(), None, nth);
        let source = packets(input, &limits, Some(kept.stream_index), 0);
        assert_eq!(written.len(), kept.samples as usize);
        assert_eq!(written.len(), source.len());
        for (a, b) in source.iter().zip(&written) {
            assert_eq!(b.duration, a.duration);
            assert_eq!(b.cts - b.dts, a.cts - a.dts);
            assert_eq!(b.keyframe, a.keyframe);
            assert_eq!(bytes(&out, b), bytes(input, a));
        }
    }
});

/// The track kinds sanitization keeps (`docs/design/sanitize.md`).
const KINDS: [(&str, &[&str]); 2] = [
    (
        "urn:mpeg:dash:role:2011",
        &[
            "caption",
            "subtitle",
            "main",
            "alternate",
            "supplementary",
            "commentary",
            "dub",
            "description",
            "sign",
            "metadata",
            "enhanced-audio-intelligibility",
            "emergency",
            "forced-subtitle",
            "easyreader",
            "karaoke",
        ],
    ),
    (
        "about:html-kind",
        &[
            "alternative",
            "captions",
            "descriptions",
            "main",
            "main-desc",
            "sign",
            "subtitles",
            "translation",
            "commentary",
        ],
    ),
];

/// ` 00000000 DDDDDDDD PPPPPPPP LLLLLLLLLLLLLLLL` and eight ` 00000000`, the
/// digits upper-case hexadecimal.
fn canonical_gapless(value: &[u8]) -> bool {
    let fields: Vec<&[u8]> = value.split(|b| *b == b' ').collect();
    let hex = |f: &[u8], n: usize| {
        f.len() == n
            && f.iter()
                .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(b))
    };
    fields.len() == 13
        && fields[0].is_empty()
        && fields[1] == b"00000000"
        && hex(fields[2], 8)
        && hex(fields[3], 8)
        && hex(fields[4], 16)
        && fields[5..].iter().all(|f| *f == b"00000000")
}

fn bx(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    [&(payload.len() as u32 + 8).to_be_bytes()[..], kind, payload].concat()
}

/// The `meta` payload sanitization writes for gapless information.
fn gapless_meta(value: &[u8]) -> Vec<u8> {
    let item = [
        bx(b"mean", b"\0\0\0\0com.apple.iTunes"),
        bx(b"name", b"\0\0\0\0iTunSMPB"),
        bx(b"data", &[&[0, 0, 0, 1, 0, 0, 0, 0][..], value].concat()),
    ]
    .concat();
    [
        &[0u8; 4][..],
        &bx(b"hdlr", b"\0\0\0\0\0\0\0\0mdirappl\0\0\0\0\0\0\0\0\0"),
        &bx(b"ilst", &bx(b"----", &item)),
    ]
    .concat()
}

/// The children (type and payload) of every `udta` in the movie box and
/// its tracks, with whether it is a track's.
fn udta_children(data: &[u8]) -> Vec<(bool, Vec<([u8; 4], Vec<u8>)>)> {
    fn boxes(data: &[u8], at: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
        let mut out = Vec::new();
        let mut at = at;
        while at + 8 <= end {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            if size < 8 || at + size > end {
                break;
            }
            out.push((data[at + 4..at + 8].try_into().unwrap(), at + 8, at + size));
            at += size;
        }
        out
    }
    let mut found = Vec::new();
    for (kind, a, e) in boxes(data, 0, data.len()) {
        if &kind != b"moov" {
            continue;
        }
        let mut parents = vec![(false, a, e)];
        parents.extend(
            boxes(data, a, e)
                .into_iter()
                .filter(|b| &b.0 == b"trak")
                .map(|b| (true, b.1, b.2)),
        );
        for (in_track, a, e) in parents {
            for (_, ua, ue) in boxes(data, a, e).into_iter().filter(|b| &b.0 == b"udta") {
                found.push((
                    in_track,
                    boxes(data, ua, ue)
                        .into_iter()
                        .map(|(k, a, e)| (k, data[a..e].to_vec()))
                        .collect(),
                ));
            }
        }
    }
    found
}

/// Every box type of the movie box's tree, containers walked.
fn box_tree(data: &[u8]) -> Vec<[u8; 4]> {
    fn walk(data: &[u8], at: usize, end: usize, out: &mut Vec<[u8; 4]>) {
        let mut at = at;
        while at + 8 <= end {
            let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
            if size < 8 || at + size > end {
                return;
            }
            out.push(kind);
            if matches!(
                &kind,
                b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf"
            ) {
                walk(data, at + 8, at + size, out);
            }
            at += size;
        }
    }
    let mut out = Vec::new();
    walk(data, 0, data.len(), &mut out);
    out
}
