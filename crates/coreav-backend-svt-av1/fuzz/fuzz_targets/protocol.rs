#![no_main]

use coreav_backend_svt_av1::{model, protocol};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 65_536 {
        return;
    }
    let Ok(settings) = model::Settings::new(64, 64, (30, 1), 8, 32, model::Color::default()) else {
        return;
    };
    let Ok(limits) = protocol::Limits::new(32_768, 65_536, 16, 4096) else {
        return;
    };
    let request = protocol::Request { settings, limits };
    let Ok(retention) = model::Retention::new(settings, limits.retained_bytes(), 16, 16, 16) else {
        return;
    };
    let chunk = data.first().map_or(1, |value| usize::from(*value) + 1);
    let mut seed = Vec::new();
    if let Ok(mut writer) = protocol::OutputWriter::new(&mut seed, "4.2.0", limits) {
        if writer.begin(0, retention, settings.keyint()).is_err() {
            return;
        }
    }
    for prefix in [&[][..], seed.as_slice()] {
        let Ok(mut decoder) = protocol::Decoder::new(request, retention) else {
            return;
        };
        if decoder.push(prefix, |_| Ok(())).is_err() {
            return;
        }
        for bytes in data.chunks(chunk) {
            if decoder.push(bytes, |_| Ok(())).is_err() {
                assert!(decoder.push(&[], |_| Ok(())).is_err());
                assert!(decoder.finish().is_err());
                break;
            }
        }
        let _ = decoder.finish();
    }
    let mut wire = Vec::new();
    if request.write(&mut wire).is_err() {
        return;
    }
    wire.extend_from_slice(data);
    if let Ok(mut input) = protocol::InputReader::new(wire.as_slice()) {
        while let Ok(Some(_)) = input.next_frame_header() {
            match input.read_picture() {
                Ok(picture) => {
                    assert_eq!(picture.len(), settings.raw_bytes());
                    assert_eq!(picture.capacity(), settings.raw_bytes());
                }
                Err(_) => {
                    assert!(input.next_frame_header().is_err());
                    break;
                }
            }
        }
    }
});
