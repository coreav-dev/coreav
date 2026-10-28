//! The portable kernel, safe Rust: the reference the vector kernels equal.

use crate::{Checked, LANES};

/// See [`crate::Fir::run`].
pub(crate) fn run(checked: Checked<'_>, out: &mut [f32]) {
    let (input, table, taps) = (checked.input(), checked.table(), checked.taps());
    for (o, &(start, row)) in out.iter_mut().zip(checked.windows()) {
        let x = input.get(start..start + taps).unwrap_or_default();
        let h = table.get(row..row + taps).unwrap_or_default();
        *o = dot(x, h);
    }
}

/// Eight lanes of products, lane `i` taking elements `i`, `i + 8`, ..., then
/// the lanes summed as the vector kernels' horizontal additions do.
#[inline(always)]
pub(crate) fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = [0.0f32; LANES];
    for (x, y) in a.chunks_exact(LANES).zip(b.chunks_exact(LANES)) {
        for ((s, x), y) in acc.iter_mut().zip(x).zip(y) {
            *s += x * y;
        }
    }
    let [l0, l1, l2, l3, l4, l5, l6, l7] = acc;
    ((l0 + l1) + (l2 + l3)) + ((l4 + l5) + (l6 + l7))
}
