#![no_main]

use coreav_backend_svt_av1::obu::{Error, Limits, Obus};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_packet_bytes: 4096,
        max_obus: data.first().copied().map_or(0, usize::from),
    };
    let mut reader = match Obus::new(data, limits) {
        Ok(reader) => reader,
        Err(error) => {
            assert_eq!(error, Error::PacketTooLarge);
            assert!(data.len() > limits.max_packet_bytes);
            return;
        }
    };
    let mut count = 0;
    for result in reader.by_ref() {
        let Ok(obu) = result else {
            break;
        };
        count += 1;
        assert!(count <= limits.max_obus);
        assert!(obu.payload().len() <= data.len());
        assert!(obu.temporal_id() < 8);
        assert!(obu.spatial_id() < 4);
    }
    assert!(reader.next().is_none());
    assert!(reader.next().is_none());
});
