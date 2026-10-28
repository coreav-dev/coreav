use super::*;
use crate::{Bandwidth, Mode, Packet};

/// The packets of a `.bit` file of the RFC 8251 vectors, with the final
/// range the reference decoder ends each in (as in `tests/vectors.rs`).
fn packets(data: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some((len, tail)) = rest.split_first_chunk::<4>() {
        let (range, tail) = tail.split_first_chunk::<4>().unwrap();
        let (packet, tail) = tail.split_at(u32::from_be_bytes(*len) as usize);
        out.push((u32::from_be_bytes(*range), packet));
        rest = tail;
    }
    out
}

fn rate(bandwidth: Bandwidth) -> Rate {
    match bandwidth {
        Bandwidth::Narrow => Rate::Narrow,
        Bandwidth::Medium => Rate::Medium,
        _ => Rate::Wide,
    }
}

/// Every SILK-only packet of the vectors that have them ends in the range
/// state the `.bit` file records (RFC 6716 section 6), but for frames with
/// redundancy, whose final state is the CELT layer's. The counts are
/// pinned, so that a misread cannot pass as redundancy: the 12 packets of
/// vector 12 with redundancy come in pairs around its SILK bandwidth
/// switches.
#[test]
#[ignore = "needs COREAV_OPUS_VECTORS"]
fn rfc8251_vectors_silk_final_range() {
    let dir = std::path::PathBuf::from(std::env::var_os("COREAV_OPUS_VECTORS").unwrap());
    let mut silk = Silk::default();
    let mut left = vec![0i16; MAX_SAMPLES];
    // (vector, packets checked, with redundancy, not SILK-only)
    for (n, counts) in [
        (2, (1185, 0, 0)),
        (3, (998, 0, 0)),
        (4, (1265, 0, 0)),
        (12, (1056, 12, 264)),
    ] {
        let name = format!("testvector{n:02}");
        let bit = std::fs::read(dir.join(format!("{name}.bit"))).unwrap();
        let (mut checked, mut redundant, mut skipped) = (0, 0, 0);
        for (i, (expected, data)) in packets(&bit).into_iter().enumerate() {
            let packet = Packet::parse(data).unwrap();
            let toc = packet.toc();
            if toc.mode() != Mode::Silk {
                skipped += 1;
                continue;
            }
            let ms = toc.frame_samples() / 48;
            let layout = Layout::new(rate(toc.bandwidth()), ms, toc.stereo()).unwrap();
            let mut last = None;
            for frame in packet.frames() {
                let mut rd = RangeDecoder::new(frame);
                silk.decode(&mut rd, layout, &mut left, None);
                let has_redundancy = rd.tell() + 17 <= 8 * frame.len() as u32;
                last = Some((rd.range(), has_redundancy, frame.len()));
            }
            match last {
                Some((_, true, _)) => redundant += 1,
                Some((range, false, len)) if len > 1 => {
                    assert_eq!(range, expected, "{name} packet {i}");
                    checked += 1;
                }
                _ => skipped += 1,
            }
        }
        println!("{name}: {checked} checked, {redundant} with redundancy, {skipped} skipped");
        assert_eq!((checked, redundant, skipped), counts, "{name}");
    }
}
