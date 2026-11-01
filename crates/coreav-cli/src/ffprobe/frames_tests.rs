#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;

struct SelectedPayloads;

impl PacketSource for SelectedPayloads {
    fn visit(
        &mut self,
        payloads: &[bool],
        f: &mut dyn FnMut(Packet) -> Result<bool, Failure>,
    ) -> Result<(), Failure> {
        assert_eq!(payloads, [false, true, false]);
        f(Packet {
            stream: 1,
            pts: Some(0),
            dts: Some(0),
            size: 4,
            data: vec![0; 4],
            ..Packet::default()
        })?;
        Ok(())
    }
}

#[test]
fn frames_request_payloads_only_for_selected_decoders() {
    let stream = |kind, codec| Stream {
        kind,
        attached_pic: false,
        entries: vec![
            ("codec_name", model::s(codec)),
            ("channels", Value::Int(2)),
            ("sample_rate", model::s("48000")),
        ],
        disposition: Vec::new(),
        tags: Vec::new(),
        side_data: Vec::new(),
    };
    let report = Report {
        streams: vec![
            stream(StreamKind::Video, "h264"),
            stream(StreamKind::Audio, "pcm_s16le"),
            stream(StreamKind::Data, "bin_data"),
        ],
        time_bases: vec![(1, 48000); 3],
        ..Report::default()
    };
    let options = args::parse(&[
        "-select_streams".into(),
        "a".into(),
        "-show_frames".into(),
        "input.mp4".into(),
    ])
    .unwrap();
    let mut probed = Probed {
        report,
        packets: Box::new(SelectedPayloads),
    };
    let mut out = Vec::new();
    assert!(emit(&options, &mut probed, &mut out).is_ok());
    assert!(String::from_utf8(out).unwrap().contains("nb_samples=1"));
}
