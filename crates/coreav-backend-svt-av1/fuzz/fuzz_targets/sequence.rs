#![no_main]

use coreav_backend_svt_av1::{obu, sequence};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 4096 {
        return;
    }
    let mut packet = vec![0x0a];
    let mut size = data.len();
    loop {
        let byte = (size & 0x7f) as u8;
        size >>= 7;
        packet.push(byte | if size == 0 { 0 } else { 0x80 });
        if size == 0 {
            break;
        }
    }
    packet.extend_from_slice(data);
    let limits = sequence::Limits {
        max_payload_bytes: 4096,
        max_width: 3840,
        max_height: 2160,
    };
    let framed = obu::Obus::new(
        &packet,
        obu::Limits {
            max_packet_bytes: 4100,
            max_obus: 1,
        },
    );
    assert!(framed.is_ok());
    let Ok(mut reader) = framed else {
        return;
    };
    let first = reader.next();
    assert!(matches!(first, Some(Ok(_))));
    let Some(Ok(obu)) = first else {
        return;
    };
    assert!(reader.next().is_none());
    if let Ok(sequence) = sequence::Sequence::parse(obu, limits) {
        assert!((1..=limits.max_width).contains(&sequence.max_width()));
        assert!((1..=limits.max_height).contains(&sequence.max_height()));
        let [marker, level, format, delay] = sequence.av1c_header().bytes();
        assert_eq!(marker, 0x81);
        assert_eq!(level, sequence.first_level());
        assert_eq!(format & 0x7c, 0x0c);
        assert_eq!(format >> 7, u8::from(sequence.first_high_tier()));
        assert_eq!(delay, 0);
        if let Some(timing) = sequence.timing() {
            assert_ne!(timing.display_tick(), 0);
            assert_ne!(timing.time_scale(), 0);
            assert_ne!(timing.ticks_per_picture(), Some(0));
        }
    }
});
