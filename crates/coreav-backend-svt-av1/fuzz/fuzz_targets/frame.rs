#![no_main]

use coreav_backend_svt_av1::{frame, obu, sequence};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() > 16_384 {
        return;
    }
    let seed = [10, 13, 0, 0, 0, 63, 248, 1, 248, 1, 248, 4, 16, 0, 128];
    let Ok(mut obus) = obu::Obus::new(
        &seed,
        obu::Limits {
            max_packet_bytes: seed.len(),
            max_obus: 1,
        },
    ) else {
        return;
    };
    let Some(Ok(obu)) = obus.next() else {
        return;
    };
    let Ok(sequence) = sequence::Sequence::parse(
        obu,
        sequence::Limits {
            max_payload_bytes: 4096,
            max_width: 64,
            max_height: 64,
        },
    ) else {
        return;
    };
    let Ok(mut frames) = frame::Frames::new(
        sequence,
        frame::Expected {
            width: 64,
            height: 64,
        },
        64,
    ) else {
        return;
    };
    let Ok(obus) = obu::Obus::new(
        data,
        obu::Limits {
            max_packet_bytes: 16_384,
            max_obus: 256,
        },
    ) else {
        return;
    };
    for obu in obus {
        let Ok(obu) = obu else {
            break;
        };
        match frames.push(obu) {
            Ok(Some(picture)) => {
                assert_eq!(
                    picture.dimensions,
                    frame::Expected {
                        width: 64,
                        height: 64
                    }
                );
                assert!(!picture.random_access || (picture.displayed && !picture.show_existing));
            }
            Ok(None) => assert!(frames.incomplete()),
            Err(_) => {
                assert_eq!(frames.push(obu), Err(frame::Error::Failed));
                assert_eq!(frames.finish(), Err(frame::Error::Failed));
                break;
            }
        }
    }
    let _ = frames.finish();
});
