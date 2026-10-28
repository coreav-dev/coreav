//! The boundary strengths of a macroblock's luma edges (clause 8.7.2.1)
//! in a frame picture: what the decoder hands [`crate::Kernels::strengths`]
//! and the reference derivation.
//!
//! Design (`docs/design/h264.md`, "Kernel round: boundary strengths in
//! vector registers"): the derivation works on masks of the sixteen blocks
//! rather than on one segment at a time. bS = 2 where either block holds a
//! coefficient, else 1 or 0 by the motion; intra macroblocks override
//! both. Most boundaries settle on the masks without a branch: motion is
//! compared only inside a macroblock that does not carry one motion
//! throughout, and only motion that differs goes through
//! [`motion_strength`], a segment at a time. The vector kernel of the
//! AVX-512 tier compares the motion of every boundary at once instead.

/// The motion of one 4x4 luma block, as the decoder keeps it with its
/// picture: the next pictures' direct prediction reads it (clause 8.4.1.2)
/// and the deblocking filter derives boundary strengths from it
/// (clause 8.7.2.1).
///
/// Twelve bytes in this order, with no padding, so that the vector kernel
/// reads a macroblock's sixteen as 192 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Motion {
    /// The motion vector of each list, in quarter luma samples.
    pub mv: [[i16; 2]; 2],
    /// The reference index of each list; negative where the list is
    /// unused, and for both lists of an intra block.
    pub ref_idx: [i8; 2],
    /// Which picture each list refers to, by a number no other picture
    /// the decoder holds has; read only for a list that is used.
    pub ref_slot: [u8; 2],
}

const _: () = assert!(size_of::<Motion>() == 12);

/// The motion of a macroblock's sixteen 4x4 blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motions<'a> {
    /// One motion for every block.
    One(&'a Motion),
    /// The motion of each block, in raster order.
    Each(&'a [Motion; 16]),
}

impl<'a> Motions<'a> {
    /// The motion of block `i` (raster order, taken modulo 16).
    #[inline(always)]
    pub(crate) fn block(self, i: usize) -> &'a Motion {
        match self {
            Motions::One(m) => m,
            Motions::Each(blocks) => match blocks.get(i & 15) {
                Some(m) => m,
                None => &blocks[0],
            },
        }
    }

    /// The one motion of a macroblock that carries one; the first block's
    /// of one that does not.
    #[inline(always)]
    fn head(self) -> &'a Motion {
        match self {
            Motions::One(m) => m,
            Motions::Each(blocks) => &blocks[0],
        }
    }

    /// Whether the macroblock carries one motion throughout.
    #[inline(always)]
    pub(crate) fn is_one(self) -> bool {
        matches!(self, Motions::One(_))
    }
}

/// A macroblock as the strength derivation reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrengthSide<'a> {
    /// Its blocks' motion.
    pub motion: Motions<'a>,
    /// The 4x4 blocks, bit `i` for block `i` in raster order, whose
    /// transform block holds a non-zero coefficient.
    pub nz: u16,
    /// Whether it is intra coded.
    pub intra: bool,
}

/// A macroblock and the neighbours its edges 0 are filtered against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrengthMb<'a> {
    /// The macroblock whose edges these are (the q side of each).
    pub q: StrengthSide<'a>,
    /// The macroblock to the left, where its vertical edge 0 is filtered.
    pub left: Option<StrengthSide<'a>>,
    /// The macroblock above, where its horizontal edge 0 is filtered.
    pub top: Option<StrengthSide<'a>>,
    /// Whether `q` uses the 8x8 transform: its edges 1 and 3 are not
    /// filtered.
    pub t8x8: bool,
}

impl StrengthMb<'_> {
    /// Whether the macroblock and its neighbours carry one motion each.
    #[inline(always)]
    pub(crate) fn all_one(&self) -> bool {
        let one = |s: Option<&StrengthSide<'_>>| s.is_none_or(|s| s.motion.is_one());
        self.q.motion.is_one() && one(self.left.as_ref()) && one(self.top.as_ref())
    }
}

/// bS of a boundary between two blocks that are neither intra nor hold
/// coefficients: 1 where their motion differs enough, else 0
/// (clause 8.7.2.1). A list is used where its reference index is not
/// negative, and only a used list's picture and vector are compared;
/// blocks that use no list on either side give 0.
///
/// Two blocks with the same motion are the common case and are answered
/// by [`same_motion`] where the masks are built; this comparison of
/// differing motion is not inlined there.
#[inline(never)]
pub(crate) fn motion_strength(p: &Motion, q: &Motion) -> u8 {
    let used = |m: &Motion| usize::from(m.ref_idx[0] >= 0) + usize::from(m.ref_idx[1] >= 0);
    let (np, nq) = (used(p), used(q));
    if np != nq {
        return 1;
    }
    let far = |a: [i16; 2], b: [i16; 2]| {
        (i32::from(a[0]) - i32::from(b[0])).abs() >= 4
            || (i32::from(a[1]) - i32::from(b[1])).abs() >= 4
    };
    if np == 0 {
        return 0;
    }
    if np == 1 {
        let list = |m: &Motion| usize::from(m.ref_idx[0] < 0);
        let (lp, lq) = (list(p), list(q));
        let (idp, idq) = (
            p.ref_slot.get(lp).copied().unwrap_or(0),
            q.ref_slot.get(lq).copied().unwrap_or(0),
        );
        if idp != idq {
            return 1;
        }
        let (mp, mq) = (
            p.mv.get(lp).copied().unwrap_or([0, 0]),
            q.mv.get(lq).copied().unwrap_or([0, 0]),
        );
        return u8::from(far(mp, mq));
    }
    // Two vectors each: the pictures must match as a set.
    let (p0, p1) = (p.ref_slot[0], p.ref_slot[1]);
    let (q0, q1) = (q.ref_slot[0], q.ref_slot[1]);
    let same_set = (p0 == q0 && p1 == q1) || (p0 == q1 && p1 == q0);
    if !same_set {
        return 1;
    }
    if p0 != p1 {
        // Two different pictures: compare the vectors that refer to each.
        if p0 == q0 {
            u8::from(far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1]))
        } else {
            u8::from(far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0]))
        }
    } else {
        // Both vectors refer to the same picture: the boundary is filtered
        // only if neither pairing is close enough.
        let straight = far(p.mv[0], q.mv[0]) || far(p.mv[1], q.mv[1]);
        let crossed = far(p.mv[0], q.mv[1]) || far(p.mv[1], q.mv[0]);
        u8::from(straight && crossed)
    }
}

/// Whether two blocks hold the same motion, as `==` but without the
/// branches of a short-circuiting comparison. Two blocks that do give
/// bS 0 by their motion.
///
/// The motion as two words, its vectors and its references, which the
/// compiler reads with one load each: one exclusive or a word and one
/// test, where `==` field by field compares the vectors and then the four
/// reference bytes in a vector register, with a branch after each.
#[inline(always)]
pub(crate) fn same_motion(p: &Motion, q: &Motion) -> bool {
    key(p) == key(q)
}

/// A block's motion as one number, for [`same_motion`].
#[inline(always)]
fn key(m: &Motion) -> u128 {
    let [[a, b], [c, d]] = m.mv;
    let [i, j] = m.ref_idx;
    let [s, t] = m.ref_slot;
    let half = |x: i16| u128::from(x as u16);
    let byte = |x: u8| u128::from(x);
    half(a)
        | half(b) << 16
        | half(c) << 32
        | half(d) << 48
        | byte(i as u8) << 64
        | byte(j as u8) << 72
        | byte(s) << 80
        | byte(t) << 88
}

/// Spreads the four low bits of `m` over the four bytes of a word, bit `s`
/// to the low bit of byte `s` (little-endian), so that a mask of segments
/// becomes a word of boundary strengths 0 or 1.
#[inline(always)]
fn spread(m: u32) -> u32 {
    // m + m << 7 + m << 14 + m << 21: bit s lands on bit 8s, and no two
    // terms meet on a bit, so nothing carries.
    (m & 0xf).wrapping_mul(0x0020_4081) & 0x0101_0101
}

/// The four bits of a 16-bit block mask (raster order) at column `e`, as
/// segments 0 to 3 of a vertical edge.
#[inline(always)]
fn column(x: u32, e: u32) -> u32 {
    let y = (x >> e) & 0x1111;
    // Bits 0, 4, 8 and 12 to bits 9 to 12: bit 4a times bit 3(3 - a) of
    // the multiplier, no two products on the same bit.
    (y.wrapping_mul(0x249) >> 9) & 0xf
}

/// The four bits of a 16-bit block mask at row `e`, as the segments of a
/// horizontal edge.
#[inline(always)]
fn row(x: u32, e: u32) -> u32 {
    (x >> (4 * e)) & 0xf
}

/// The reference derivation of [`crate::Kernels::strengths`].
///
/// Where the macroblock and its neighbours all carry one motion, the
/// derivation runs without any path that reads blocks; where one of them
/// carries more, [`split_strengths`] reads them. Where both macroblocks of
/// edge 0 carry one motion, its four segments compare the same two
/// motions, so one comparison answers for all four.
#[inline(always)]
pub(crate) fn strengths(mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
    // Edges 1 and 3 of a macroblock with the 8x8 transform are not edges.
    let edges = if mb.t8x8 { 0b0101 } else { 0b1111 };
    if !mb.all_one() {
        return split_strengths(mb, edges);
    }
    uniform_strengths(mb)
}

/// [`crate::Kernels::strengths`] where the macroblock and its neighbours
/// carry one motion each ([`Motions::One`]), for every tier: a few masks
/// and two comparisons of motion, which the caller inlines. The kernels'
/// call is for the macroblocks that carry more.
#[inline(always)]
pub fn uniform_strengths(mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
    debug_assert!(mb.all_one());
    let edges = if mb.t8x8 { 0b0101 } else { 0b1111 };
    [
        edge_strengths::<0, false>(&mb.q, mb.left.as_ref(), edges),
        edge_strengths::<1, false>(&mb.q, mb.top.as_ref(), edges),
    ]
}

/// [`strengths`] where the macroblock or a neighbour carries more than one
/// motion. Out of line: most macroblocks and their neighbours carry one,
/// and their path keeps fewer values live without the loops over blocks.
#[inline(never)]
fn split_strengths(mb: &StrengthMb<'_>, edges: u32) -> [[u32; 4]; 2] {
    [
        edge_strengths::<0, true>(&mb.q, mb.left.as_ref(), edges),
        edge_strengths::<1, true>(&mb.q, mb.top.as_ref(), edges),
    ]
}

/// [`strengths`] in one direction: `DIR` 0 for the vertical edges, whose
/// segments are the rows of a column of blocks, 1 for the horizontal ones.
#[inline(always)]
fn edge_strengths<const DIR: usize, const SPLIT: bool>(
    q: &StrengthSide<'_>,
    p: Option<&StrengthSide<'_>>,
    edges: u32,
) -> [u32; 4] {
    // The segments of edge `e` in a block mask, as four bits.
    let lines = |x: u32, e: u32| if DIR == 0 { column(x, e) } else { row(x, e) };
    // The distance in raster order from a block to the one across an edge.
    let step: usize = if DIR == 0 { 1 } else { 4 };
    // The internal boundaries in a block mask, bit `i` for the one before
    // block `i`: every block but the first column's or the first row's.
    let inside: u32 = if DIR == 0 { 0xeeee } else { 0xfff0 };
    let q_nz = u32::from(q.nz);
    let mut out = [0u32; 4];
    if let Some(p) = p {
        out[0] = if q.intra || p.intra {
            0x0404_0404
        } else {
            let coef = lines(q_nz, 0) | lines(u32::from(p.nz), 3);
            let mut moved = 0;
            if !SPLIT || (q.motion.is_one() && p.motion.is_one()) {
                // One motion on each side: one comparison for the edge.
                let (pm, qm) = (p.motion.head(), q.motion.head());
                if coef != 0xf && !same_motion(pm, qm) {
                    moved = 0xf * u32::from(motion_strength(pm, qm));
                }
            } else {
                moved = split_edge::<DIR>(q.motion, p.motion, coef);
            }
            (spread(coef) << 1) | spread(moved & !coef)
        };
    }
    if q.intra {
        for (e, w) in out.iter_mut().enumerate().skip(1) {
            *w = 0x0303_0303 * ((edges >> e) & 1);
        }
        return out;
    }
    // bS = 2 where either block of an internal boundary holds a
    // coefficient: the block's own bit, or the bit of the block before it.
    let coef = (q_nz | (q_nz << step)) & inside;
    // Where the motion is uniform, every other internal boundary is 0.
    let moved = match q.motion {
        Motions::Each(blocks) if SPLIT => split_inside::<DIR>(blocks, coef),
        _ => 0,
    };
    // No internal boundary with a coefficient or differing motion: every
    // internal edge is 0, which `out` already holds. The common case where
    // macroblocks are skipped, and the words below are not built.
    if coef | moved == 0 {
        return out;
    }
    for (e, w) in (1u32..).zip(out.iter_mut().skip(1)) {
        let on = 0u32.wrapping_sub((edges >> e) & 1);
        *w = ((spread(lines(coef, e)) << 1) | spread(lines(moved, e))) & on;
    }
    out
}

/// The segments of edge 0 in direction `DIR` whose motion gives bS = 1,
/// as four bits, where `q` or `p` carries more than one motion; `coef`
/// marks the segments settled by a coefficient.
#[inline(always)]
fn split_edge<const DIR: usize>(q: Motions<'_>, p: Motions<'_>, coef: u32) -> u32 {
    // Segment `s` of edge `e`, as a raster block index.
    let at = |e: usize, s: usize| if DIR == 0 { s * 4 + e } else { e * 4 + s };
    let mut moved = 0;
    for s in 0..4 {
        let (qm, pm) = (q.block(at(0, s)), p.block(at(3, s)));
        if (coef >> s) & 1 == 0 && !same_motion(pm, qm) {
            moved |= u32::from(motion_strength(pm, qm)) << s;
        }
    }
    moved
}

/// The internal boundaries in direction `DIR` of a macroblock with the
/// sixteen blocks `blocks` whose motion gives bS = 1, bit `i` for the one
/// before block `i`; `coef` marks those settled by a coefficient.
#[inline(always)]
fn split_inside<const DIR: usize>(blocks: &[Motion; 16], coef: u32) -> u32 {
    let step: usize = if DIR == 0 { 1 } else { 4 };
    let inside: u32 = if DIR == 0 { 0xeeee } else { 0xfff0 };
    let mut differs = 0u32;
    for (i, (pm, qm)) in blocks.iter().zip(blocks.iter().skip(step)).enumerate() {
        differs |= u32::from(!same_motion(pm, qm)) << (i + step);
    }
    let mut moved = 0;
    let mut pending = differs & !coef & inside;
    while pending != 0 {
        let i = pending.trailing_zeros() as usize;
        pending &= pending - 1;
        let pair = blocks.get(i.wrapping_sub(step)).zip(blocks.get(i));
        if let Some((pm, qm)) = pair {
            moved |= u32::from(motion_strength(pm, qm)) << i;
        }
    }
    moved
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The derivation a segment at a time, as clause 8.7.2.1 reads, with
    /// the motion of every block of the three macroblocks: the reference
    /// the masks and the vector kernel are checked against.
    pub(crate) fn by_segment(mb: &StrengthMb<'_>) -> [[u32; 4]; 2] {
        let motion_strength = |p: &Motion, q: &Motion| {
            if p == q { 0 } else { motion_strength(p, q) }
        };
        let q = &mb.q;
        let mut out = [[[0u8; 4]; 4]; 2];
        for (dir, edges) in out.iter_mut().enumerate() {
            let neighbour = if dir == 0 { mb.left } else { mb.top };
            if let Some(p) = neighbour {
                for (s, bs) in edges[0].iter_mut().enumerate() {
                    let (qr, pr) = if dir == 0 {
                        (s * 4, s * 4 + 3)
                    } else {
                        (s, 12 + s)
                    };
                    *bs = if q.intra || p.intra {
                        4
                    } else if (p.nz >> pr) & 1 != 0 || (q.nz >> qr) & 1 != 0 {
                        2
                    } else {
                        motion_strength(p.motion.block(pr), q.motion.block(qr))
                    };
                }
            }
            for (e, edge) in edges.iter_mut().enumerate().skip(1) {
                if mb.t8x8 && e % 2 == 1 {
                    continue;
                }
                for (s, bs) in edge.iter_mut().enumerate() {
                    let (qr, pr) = if dir == 0 {
                        (s * 4 + e, s * 4 + e - 1)
                    } else {
                        (e * 4 + s, e * 4 + s - 4)
                    };
                    *bs = if q.intra {
                        3
                    } else if ((q.nz >> pr) | (q.nz >> qr)) & 1 != 0 {
                        2
                    } else {
                        motion_strength(q.motion.block(pr), q.motion.block(qr))
                    };
                }
            }
        }
        out.map(|dir| dir.map(u32::from_le_bytes))
    }

    pub(crate) struct Rng(pub(crate) u64);

    impl Rng {
        pub(crate) fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// A motion from a small set, so that equal, close and distant motion,
    /// every count of lists, and two indices of one picture all occur.
    /// With `wild`, also what a decoder never stores: a vector, picture
    /// or index in a list that is not used, vectors far apart past the
    /// range of 16 bits, and blocks that use no list.
    pub(crate) fn motion(rng: &mut Rng, wild: bool) -> Motion {
        let list = |rng: &mut Rng, used: bool| {
            let mv = if wild && rng.next(4) == 0 {
                let big = [i16::MIN, i16::MAX, -3, 3, 4, -4, 0];
                [big[rng.next(7) as usize], big[rng.next(7) as usize]]
            } else {
                [rng.next(3) as i16 * 3 - 3, rng.next(3) as i16 * 3]
            };
            if used {
                let r = rng.next(3) as i8;
                // Indices 1 and 2 may name the same picture.
                let slot = [10, 11, 11][r as usize] + 20 * (rng.next(2) as u8);
                (r, slot, mv)
            } else if wild {
                (-1 - rng.next(2) as i8, rng.next(256) as u8, mv)
            } else {
                (-1, 255, [0, 0])
            }
        };
        let lists = rng.next(if wild { 4 } else { 3 });
        let a = list(rng, lists == 0 || lists == 2);
        let b = list(rng, lists == 1 || lists == 2);
        Motion {
            mv: [a.2, b.2],
            ref_idx: [a.0, b.0],
            ref_slot: [a.1, b.1],
        }
    }

    /// Three macroblocks' motion (left, top, current), nz, intra flags and
    /// layouts, as `strengths` takes them; `blocks` holds the sixteen
    /// blocks of each, of which a macroblock that carries one motion uses
    /// the first.
    pub(crate) struct Case {
        pub(crate) blocks: [[Motion; 16]; 3],
        pub(crate) one: [bool; 3],
        pub(crate) nz: [u16; 3],
        pub(crate) intra: [bool; 3],
        pub(crate) left: bool,
        pub(crate) top: bool,
        pub(crate) t8x8: bool,
    }

    impl Case {
        pub(crate) fn random(rng: &mut Rng, wild: bool) -> Case {
            let mut case = Case {
                blocks: [[Motion::default(); 16]; 3],
                one: [true; 3],
                nz: [0; 3],
                intra: [false; 3],
                left: rng.next(4) != 0,
                top: rng.next(4) != 0,
                t8x8: rng.next(4) == 0,
            };
            for k in 0..3 {
                case.intra[k] = rng.next(6) == 0;
                case.nz[k] = match rng.next(3) {
                    0 => 0,
                    1 => 1 << rng.next(16),
                    _ => rng.next(1 << 16) as u16,
                };
                let blocks = &mut case.blocks[k];
                let first = motion(rng, wild);
                blocks.fill(first);
                // Some macroblocks carry one motion, the others a motion
                // per 8x8 quarter, per row of blocks, or per block.
                match rng.next(4) {
                    0 => {}
                    1 => {
                        for (i, b) in blocks.iter_mut().enumerate() {
                            if (i % 4) / 2 + 2 * (i / 8) == 3 {
                                *b = motion(rng, wild);
                            }
                        }
                    }
                    2 => {
                        let other = motion(rng, wild);
                        blocks[4..8].fill(other);
                    }
                    _ => {
                        for b in blocks.iter_mut() {
                            if rng.next(3) == 0 {
                                *b = motion(rng, wild);
                            }
                        }
                    }
                }
                // One that carries one motion may also be stored block by
                // block.
                case.one[k] = blocks.iter().all(|b| b == &first) && rng.next(4) != 0;
            }
            case
        }

        pub(crate) fn mb(&self) -> StrengthMb<'_> {
            let side = |k: usize| StrengthSide {
                motion: if self.one[k] {
                    Motions::One(&self.blocks[k][0])
                } else {
                    Motions::Each(&self.blocks[k])
                },
                nz: self.nz[k],
                intra: self.intra[k],
            };
            StrengthMb {
                q: side(2),
                left: self.left.then(|| side(0)),
                top: self.top.then(|| side(1)),
                t8x8: self.t8x8,
            }
        }
    }

    #[test]
    fn the_masks_match_the_derivation_per_segment() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for n in 0..300_000 {
            let case = Case::random(&mut rng, n % 2 == 1);
            let mb = case.mb();
            assert_eq!(strengths(&mb), by_segment(&mb), "{mb:?}");
        }
    }

    /// Every tier against the derivation per segment, on the motion a
    /// decoder stores and on motion it never does (`motion`'s `wild`).
    #[test]
    fn every_tier_matches_the_derivation_per_segment() {
        use crate::{Auto, Kernels, Scalar};
        type Tier = Box<dyn Fn(&StrengthMb<'_>) -> [[u32; 4]; 2]>;
        let mut tiers: Vec<(&str, Tier)> = vec![
            ("scalar", Box::new(|mb| Scalar.strengths(mb))),
            ("auto", Box::new(|mb| Auto::detect().strengths(mb))),
        ];
        if let Some(simd) = crate::Simd::detect() {
            tiers.push(("simd", Box::new(move |mb| simd.strengths(mb))));
            let narrow = simd.without_avx512();
            tiers.push((
                "simd without avx512",
                Box::new(move |mb| narrow.strengths(mb)),
            ));
        }
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for n in 0..300_000 {
            let case = Case::random(&mut rng, n % 2 == 1);
            let mb = case.mb();
            let want = by_segment(&mb);
            for (name, tier) in &tiers {
                assert_eq!(tier(&mb), want, "{name} {mb:?}");
            }
        }
    }

    #[test]
    fn spread_and_column_place_every_bit() {
        for m in 0..16u32 {
            let bytes = spread(m).to_le_bytes();
            for (s, b) in bytes.iter().enumerate() {
                assert_eq!(u32::from(*b), (m >> s) & 1);
            }
        }
        for x in 0..1u32 << 16 {
            for e in 0..4 {
                let want = (0..4).fold(0, |a, s| a | (((x >> (s * 4 + e)) & 1) << s));
                assert_eq!(column(x, e), want);
            }
        }
    }

    /// The rule of [`motion_strength`] where both blocks use the lists the
    /// clause describes, written as the clause words it, against a few
    /// hand-made pairs.
    #[test]
    fn motion_strength_follows_the_clause() {
        let m = |mv: [[i16; 2]; 2], ref_idx: [i8; 2], ref_slot: [u8; 2]| Motion {
            mv,
            ref_idx,
            ref_slot,
        };
        // One list each, the same picture through different indices.
        let a = m([[0, 0], [0, 0]], [0, -1], [7, 255]);
        let b = m([[3, -3], [0, 0]], [2, -1], [7, 255]);
        assert_eq!(motion_strength(&a, &b), 0);
        // The same picture through different lists.
        let c = m([[0, 0], [3, 3]], [-1, 1], [255, 7]);
        assert_eq!(motion_strength(&a, &c), 0);
        let d = m([[0, 0], [4, 0]], [-1, 1], [255, 7]);
        assert_eq!(motion_strength(&a, &d), 1);
        // Different counts of vectors.
        let e = m([[0, 0], [0, 0]], [0, 0], [7, 7]);
        assert_eq!(motion_strength(&a, &e), 1);
        // Two vectors to one picture: either pairing close enough.
        let f = m([[0, 0], [8, 0]], [0, 0], [7, 7]);
        let g = m([[8, 0], [1, 0]], [0, 0], [7, 7]);
        assert_eq!(motion_strength(&f, &g), 0);
        let h = m([[8, 0], [4, 0]], [0, 0], [7, 7]);
        assert_eq!(motion_strength(&f, &h), 1);
        // Two pictures, crossed lists.
        let i = m([[0, 0], [8, 0]], [0, 1], [7, 9]);
        let j = m([[8, 0], [0, 0]], [0, 0], [9, 7]);
        assert_eq!(motion_strength(&i, &j), 0);
        // Vectors at the ends of the 16-bit range.
        let k = m([[i16::MIN, 0], [0, 0]], [0, -1], [7, 255]);
        let l = m([[i16::MAX, 0], [0, 0]], [0, -1], [7, 255]);
        assert_eq!(motion_strength(&k, &l), 1);
    }
}
