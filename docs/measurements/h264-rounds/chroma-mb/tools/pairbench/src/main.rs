//! pairbench MODE EDGES CALLS: MODE pair (one filter_chroma_pair a filtered edge, with its two views) or mb
//! (filter_chroma_mb with its two windows); EDGES a mask of the filtered edges (1 V0, 2 V4, 4 H0, 8 H4). Prints ns a
//! macroblock and an FNV-1a hash of both planes, which differs between edge sets that filter differently.
use coreav_kernels_h264::{Auto, ChromaMbMut, EdgeMut, EdgeParams, Kernels, Reach};
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mb = a[1] == "mb";
    let mask: usize = a[2].parse().unwrap();
    let calls: usize = a[3].parse().unwrap();
    let k = Auto::detect();
    let stride = 64;
    let mut cb: Vec<u8> = (0..stride * 64).map(|i| (100 + (i * 7 % 5)) as u8).collect();
    let mut cr: Vec<u8> = (0..stride * 64).map(|i| (90 + (i * 3 % 7)) as u8).collect();
    let p = EdgeParams { alpha: 40, beta: 10, tc0: [2, 2, 2, 2], bs: [2; 4] };
    let mut params = [[EdgeParams::default(); 2]; 4];
    for s in 0..4 {
        if mask >> s & 1 == 1 {
            params[s] = [p, p];
        }
    }
    let t = Instant::now();
    for i in 0..calls {
        let x = 8 + 8 * (i % 6);
        let y = 8 + 8 * ((i / 6) % 6);
        let at = y * stride + x;
        let params = std::hint::black_box(&params);
        if mb {
            let b = ChromaMbMut::new(&mut cb, at, stride).unwrap();
            let r = ChromaMbMut::new(&mut cr, at, stride).unwrap();
            k.filter_chroma_mb(b, r, params);
        } else {
            for (s, pair) in params.iter().enumerate() {
                if pair[0].bs == [0; 4] && pair[1].bs == [0; 4] {
                    continue;
                }
                let (q0, across, along) = match s {
                    0 => (at, 1, stride),
                    1 => (at + 4, 1, stride),
                    2 => (at, stride, 1),
                    _ => (at + 4 * stride, stride, 1),
                };
                let e1 = EdgeMut::new(&mut cb, q0, across, along, 8, Reach::Chroma).unwrap();
                let e2 = EdgeMut::new(&mut cr, q0, across, along, 8, Reach::Chroma).unwrap();
                k.filter_chroma_pair(e1, e2, pair);
            }
        }
    }
    let ns = t.elapsed().as_nanos() as f64 / calls as f64;
    println!("{} edges {:04b} {:.1} ns a macroblock (hash {:016x})", if mb { "mb  " } else { "pair" }, mask, ns,
             cb.iter().chain(cr.iter()).fold(0xcbf2_9ce4_8422_2325u64, |h, &v| (h ^ u64::from(v)).wrapping_mul(0x100_0000_01b3)));
}
