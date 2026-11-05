#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::boxes::Node;
use coreav_isobmff::SliceSource;

fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    Node::leaf(kind, payload.to_vec())
        .write(&mut bytes)
        .unwrap();
    bytes
}

fn movie(codec: &[u8; 4], fillers: usize, fiel: Option<&[u8]>) -> Vec<u8> {
    let mut entry = vec![0; 78];
    entry[42..47].copy_from_slice(b"\x04test");
    for _ in 0..fillers {
        entry.extend_from_slice(&boxed(b"free", &[]));
    }
    if let Some(payload) = fiel {
        entry.extend_from_slice(&boxed(b"fiel", payload));
    }
    let mut stsd = vec![0; 4];
    stsd.extend_from_slice(&1u32.to_be_bytes());
    stsd.extend_from_slice(&boxed(codec, &entry));
    let minf = boxed(b"minf", &boxed(b"stbl", &boxed(b"stsd", &stsd)));
    let mut handler = vec![0; 8];
    handler.extend_from_slice(b"vide");
    let mut mdia = boxed(b"hdlr", &handler);
    mdia.extend_from_slice(&minf);
    boxed(b"moov", &boxed(b"trak", &boxed(b"mdia", &mdia)))
}

#[test]
fn field_order_header_is_retained_beyond_the_entry_summary_cap() {
    for codec in [*b"av01", *b"vp09", *b"hvc1"] {
        let bytes = movie(&codec, MAX_LISTED, Some(&[2, 9]));
        let mut source = SliceSource::new(&bytes);
        let metadata = scan(&mut source, &Limits::default()).unwrap();
        let track = &metadata.traks[0];
        assert_eq!(track.entry_children.len(), MAX_LISTED);
        assert!(
            track
                .entry_children
                .iter()
                .all(|child| child.kind.0 != *b"fiel")
        );
        let header = track.fiel.unwrap();
        assert_eq!(header.payload_len(), 2);
        let mut payload = [0; 2];
        source
            .read_exact_at(header.payload_offset(), &mut payload)
            .unwrap();
        assert_eq!(payload, [2, 9]);
        assert_eq!(&track.compressor_name.unwrap()[..5], b"\x04test");
    }
}

#[test]
fn absent_and_short_field_order_payloads_do_not_add_refusals() {
    for payload in [None, Some(&[][..]), Some(&[1][..])] {
        let bytes = movie(b"av01", 0, payload);
        let metadata = scan(&mut SliceSource::new(&bytes), &Limits::default()).unwrap();
        assert!(metadata.traks[0].fiel.is_none());
    }
}

#[test]
fn retaining_field_order_does_not_bypass_the_box_budget() {
    let bytes = movie(b"av01", MAX_LISTED, Some(&[1, 0]));
    let limits = Limits {
        max_total_boxes: 16,
        ..Limits::default()
    };
    assert!(matches!(
        scan(&mut SliceSource::new(&bytes), &limits),
        Err(Error::LimitExceeded {
            limit: LimitKind::TotalBoxes,
            ..
        })
    ));
}
