//! Bar C3 for the three-plane prediction: the vector kernels and the
//! dispatch give the same answer and the same samples as the trait's
//! by-parts form through the reference, for every block size, position,
//! vector (windows inside the reference, at its edges and outside it,
//! which are declined unless only unused samples lie outside), operation
//! per plane and band of the picture, slack rows before and after each
//! plane, with planes no longer than they must be, so that a read or
//! write past a window or block leaves its slice. And the same reference
//! cut to the rows one band of it holds (`Planes::band`): every kernel
//! takes from it only blocks the whole reference takes, predicts them
//! alike, and writes nothing for the others. And the same block by its
//! macroblock (`Kernels::predict_part`): what `predict` writes, or
//! nothing.
#![no_main]

use coreav_kernels_h264::{
    Auto, Kernels, Ops, Part, PlaneOp, Planes, PlanesMut, Scalar, Simd, Weighting,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some(simd) = Simd::detect() else { return };
    let Some((head, samples)) = data.split_at_checked(19) else {
        return;
    };
    let size = |b: u8| [4, 8, 16][usize::from(b) % 3];
    let (w, h) = (size(head[0]), size(head[0] / 3));
    // A reference 2 to 64 samples wide and tall, both even.
    let stride = 2 * (usize::from(head[1] % 32) + 1);
    let rows = 2 * (usize::from(head[2] % 32) + 1);
    // The band's first luma row, and its first chroma row: half the
    // luma one, or one either side of it, as another apron would make it.
    let first = usize::from(head[3] % 8);
    let chroma_first = match head[3] / 8 % 3 {
        0 => first / 2,
        1 => first / 2 + 1,
        _ => (first / 2).saturating_sub(1),
    };
    let band = rows.saturating_sub(first).max(2);
    let chroma_band = (rows / 2).saturating_sub(chroma_first).max(1);
    // Positions inside the picture and a little past it, and vectors of
    // up to 32 samples each way, so that most windows are inside a small
    // reference and the rest reach past each of its edges.
    let px = 2 * (usize::from(head[4]) % (stride / 2 + 4));
    let py = first + 2 * (usize::from(head[5]) % (band / 2 + 4));
    let mv = [i16::from(head[6] as i8), i16::from(head[8] as i8)];
    let value = |b: u8| i32::from(b as i8) * 9 / 8;
    let op = |k: u8, a: u8, b: u8| match k % 4 {
        0 => PlaneOp::Put,
        1 => PlaneOp::Avg,
        2 => PlaneOp::Weigh(Weighting::Uni {
            log_wd: i32::from(k / 4 % 10) - 1,
            w: value(a),
            o: value(b),
        }),
        _ => PlaneOp::Weigh(Weighting::Bi {
            log_wd: i32::from(k / 4 % 10) - 1,
            w0: value(a),
            w1: value(b),
            o: value(a ^ b),
        }),
    };
    let ops = [
        op(head[10], head[11], head[12]),
        op(head[13], head[14], head[15]),
        op(head[13] / 4, head[15], head[11]),
    ];
    if samples.is_empty() {
        return;
    }
    let fill = |n: usize, skip: usize| -> Vec<u8> {
        samples.iter().cycle().skip(skip).take(n).copied().collect()
    };
    // Slack rows before each plane's origin and after its last row, in
    // the reference (up to the decoder's 4 luma and 2 chroma rows, and a
    // few samples less) and in the picture.
    let cs = stride / 2;
    let short = usize::from(head[7] / 15 % 2);
    let slack = [usize::from(head[7] % 5), usize::from(head[7] / 5 % 3)];
    let origin = [slack[0] * stride, slack[1] * cs];
    let (luma, chroma) = (
        stride * (rows + 2 * slack[0]) - short,
        cs * (rows / 2 + 2 * slack[1]) - short,
    );
    let (ry, rcb, rcr) = (fill(luma, 0), fill(chroma, 7), fill(chroma, 13));
    let Some(src) = Planes::with_origin(&ry, &rcb, &rcr, stride, rows, origin) else {
        return;
    };
    let dst_origin = [
        usize::from(head[9] % 5) * stride,
        usize::from(head[9] / 5 % 3) * cs,
    ];
    let (dy, dcb) = (
        fill(dst_origin[0] + stride * band, 3),
        fill(dst_origin[1] + cs * chroma_band, 5),
    );
    let dcr = fill(dst_origin[1] + cs * chroma_band, 11);
    let run = |kernels: &dyn Kernels| {
        let (mut y, mut cb, mut cr) = (dy.clone(), dcb.clone(), dcr.clone());
        let first = [first, chroma_first];
        let dst = PlanesMut::with_origin(&mut y, &mut cb, &mut cr, stride, first, dst_origin);
        let done = kernels.predict(dst, &src, px, py, w, h, mv, ops);
        (done, y, cb, cr)
    };
    let want = run(&Scalar);
    if !want.0 {
        assert!(
            want.1 == dy && want.2 == dcb && want.3 == dcr,
            "a declined block wrote"
        );
    }
    let auto = Auto::detect();
    let kernels: [(&str, &dyn Kernels); 4] = [
        ("simd", &simd),
        ("simd-avx2", &simd.without_avx512()),
        ("auto", &auto),
        ("auto-avx2", &auto.without_avx512()),
    ];
    for (name, k) in kernels {
        assert!(
            run(k) == want,
            "{name} {w}x{h} at ({px}, {py}), first {first} {chroma_first}, {mv:?}, {ops:?}"
        );
    }
    // The same block by its macroblock (`Kernels::predict_part`), from the
    // macroblocks the picture's planes hold: what `predict` writes, or
    // nothing.
    if let (Some(part), Some(checked)) = (Part::new(px % 16, py % 16, w, h), Ops::new(ops)) {
        for (name, k) in [("scalar", &Scalar as &dyn Kernels)].into_iter().chain(kernels) {
            let (mut y, mut cb, mut cr) = (dy.clone(), dcb.clone(), dcr.clone());
            let first = [first, chroma_first];
            let mbs = PlanesMut::with_origin(&mut y, &mut cb, &mut cr, stride, first, dst_origin)
                .macroblocks();
            // One slice, chosen by the input, shorter than the macroblocks
            // hold it to (the luma slice's length, and the shorter chroma
            // slice's), which must be declined; or none.
            let cut = usize::from(head[18] / 4).max(1);
            let chroma = cb.len().min(cr.len());
            let short = |plane: usize, n: usize, bound: usize| {
                if usize::from(head[18] % 4) == plane {
                    bound.saturating_sub(cut)
                } else {
                    n
                }
            };
            let ny = short(0, y.len(), y.len());
            let (ncb, ncr) = (short(1, cb.len(), chroma), short(2, cr.len(), chroma));
            let shortened = (ny, ncb, ncr) != (y.len(), cb.len(), cr.len());
            let dst = [&mut y[..ny], &mut cb[..ncb], &mut cr[..ncr]];
            let done = k.predict_part(dst, &mbs, [px / 16, py / 16], part, &src, mv, checked);
            let what = format!("part: {name} {w}x{h} at ({px}, {py}), {mv:?}, {mbs:?}, short {head:?}");
            if done {
                assert!(!shortened && want.0 && (done, y, cb, cr) == want, "{what}");
            } else {
                assert!(y == dy && cb == dcb && cr == dcr, "{what}: a declined block wrote");
            }
        }
    }
    // The reference's rows `held[0]..` and chroma rows `held[1]..`, as many
    // as the slices hold, from the picture's rows (not its slack).
    let held = [
        usize::from(head[16]) % rows,
        usize::from(head[17]) % (rows / 2),
    ];
    let count = [
        usize::from(head[16] / 64) * 8 + 4,
        usize::from(head[17] / 64) * 4 + 2,
    ];
    let cut = |plane: &[u8], at: usize, first: usize, n: usize, row: usize| -> Vec<u8> {
        let from = (at + first * row).min(plane.len());
        let to = (at + (first + n) * row).min(plane.len()).max(from);
        plane[from..to].to_vec()
    };
    let by = cut(&ry, origin[0], held[0], count[0], stride);
    let bcb = cut(&rcb, origin[1], held[1], count[1], cs);
    let bcr = cut(&rcr, origin[1], held[1], count[1], cs);
    let Some(band_src) = Planes::band(&by, &bcb, &bcr, stride, rows, held, [0, 0]) else {
        return;
    };
    for (name, k) in [("scalar", &Scalar as &dyn Kernels)]
        .into_iter()
        .chain(kernels)
    {
        let (mut y, mut cb, mut cr) = (dy.clone(), dcb.clone(), dcr.clone());
        let first = [first, chroma_first];
        let dst = PlanesMut::with_origin(&mut y, &mut cb, &mut cr, stride, first, dst_origin);
        let done = k.predict(dst, &band_src, px, py, w, h, mv, ops);
        let what = format!("band {held:?} {count:?}: {name} {w}x{h} at ({px}, {py}), {mv:?}");
        if done {
            assert!(want.0 && (done, y, cb, cr) == want, "{what}");
        } else {
            assert!(
                y == dy && cb == dcb && cr == dcr,
                "{what}: a declined block wrote"
            );
        }
    }
});
