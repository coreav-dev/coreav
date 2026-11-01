//! Motion of the coded macroblocks of a CAVLC P slice whose partitions are
//! 8x8 or larger: P_L0_16x16, P_L0_L0_16x8, P_L0_L0_8x16, and P_8x8 and
//! P_8x8ref0 with four 8x8 sub-macroblocks (`docs/design/h264.md`,
//! "MB layer round: the CAVLC P macroblock path").
//!
//! The slice loop reads their mb_pred() or sub_mb_pred() into a [`PPred`]
//! (`SliceDecoder::parse_p_inter`), and the macroblock's prediction derives
//! the motion from it here: each partition's neighbours A, B, C and D
//! (clause 6.4.11.7) are at positions its shape fixes, so each is one
//! lookup of a block that a neighbouring macroblock left in the picture,
//! or the motion of a partition just derived, where the generic path
//! (`SliceDecoder::inter_pred`) fills the whole [`super::MotionCtx`] grid and
//! looks it up with positions computed per partition. The directional rules
//! of 16x8 and 8x16 partitions (Equations 8-203 to 8-206) are written out
//! per partition. Nothing that only CABAC reads is written, and the
//! generic path's `MbData` fields are not used.
//!
//! The order of everything that can fail is the generic path's: per
//! partition, the vector's range, then the reference index against its
//! list, then motion compensation.
#![forbid(unsafe_code)]

use super::{UNAVAILABLE, median, put_row, to_mv};
use crate::error::{Error, Result};
use crate::mb::SliceDecoder;
use crate::picture::{MbHead, Motion, NO_SLOT};
use crate::tier::Tier;
use coreav_kernels_h264::Kernels;

/// The partitions of a macroblock [`PPred`] describes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PShape {
    /// P_L0_16x16: one partition.
    #[default]
    P16x16,
    /// P_L0_L0_16x8: two partitions, one above the other.
    P16x8,
    /// P_L0_L0_8x16: two partitions side by side.
    P8x16,
    /// P_8x8 or P_8x8ref0 whose four sub-macroblocks are each one 8x8
    /// partition (sub_mb_type 0).
    P8x8,
}

/// What mb_pred() or sub_mb_pred() of a macroblock of [`PShape`] codes:
/// per partition, in decoding order, ref_idx_l0 and mvd_l0, the latter
/// already checked against the range of clause 7.4.5.1. Written in place
/// by the slice loop and read by reference, field by field, so that it
/// stays a set of scalars rather than a value packed and moved whole.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PPred {
    pub(crate) shape: PShape,
    pub(crate) ref_idx: [i8; 4],
    pub(crate) mvd: [[i32; 2]; 4],
}

/// The reference index and vector of list 0 of a block that motion vector
/// prediction reads (clause 8.4.1.3.2): [`UNAVAILABLE`] where it is not
/// available, -1 where it is intra.
type Nb = (i8, [i16; 2]);

/// mvpL0 from neighbours A, B and C, where C is already D if C is not
/// available, for reference index `r` (clause 8.4.1.3.1): B and C take A's
/// place where neither is available and A is, a single neighbour with
/// reference index `r` gives its vector, and otherwise the median.
#[inline(always)]
fn predict(a: Nb, b: Nb, c: Nb, r: i8) -> [i16; 2] {
    let (b, c) = if b.0 == UNAVAILABLE && c.0 == UNAVAILABLE && a.0 != UNAVAILABLE {
        (a, a)
    } else {
        (b, c)
    };
    let (ma, mb, mc) = (a.0 == r, b.0 == r, c.0 == r);
    if u8::from(ma) + u8::from(mb) + u8::from(mc) == 1 {
        return if ma {
            a.1
        } else if mb {
            b.1
        } else {
            c.1
        };
    }
    [
        median(a.1[0], b.1[0], c.1[0]),
        median(a.1[1], b.1[1], c.1[1]),
    ]
}

/// The vector of a partition from its predictor and its mvd, in the range
/// of clause 7.4.5.1.
#[inline(always)]
fn add(mvp: [i16; 2], mvd: [i32; 2]) -> Result<[i16; 2]> {
    to_mv([i32::from(mvp[0]) + mvd[0], i32::from(mvp[1]) + mvd[1]])
}

impl<K: Kernels> SliceDecoder<'_, K> {
    /// Derives the motion of the current macroblock from what its syntax
    /// coded, stores it, and predicts the macroblock (clause 8.4).
    ///
    /// `T`, unused but for motion compensation, gives each instance of the
    /// hot paths its own copy ([`crate::tier`]).
    #[inline(always)]
    pub(crate) fn predict_p<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        if p.shape == PShape::P16x16 {
            self.predict_p16x16(tier, p)
        } else {
            self.predict_p_parts(tier, p)
        }
    }

    /// [`Self::predict_p`] of the shapes of more than one partition, out
    /// of line in `tier`: the code of each stays out of the slice loop.
    #[inline(never)]
    fn predict_p_parts<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        tier.enter(
            #[inline(always)]
            || self.predict_p_parts_body(tier, p),
        )
    }

    #[inline(always)]
    fn predict_p_parts_body<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        // P16x16 is taken inline by `predict_p` and does not come here; its
        // arm keeps the match total without a panic.
        match p.shape {
            PShape::P16x16 => self.predict_p16x16(tier, p),
            PShape::P16x8 => self.predict_p16x8(tier, p),
            PShape::P8x16 => self.predict_p8x16(tier, p),
            PShape::P8x8 => self.predict_p8x8(tier, p),
        }
    }

    /// List 0 of raster block `blk` of neighbouring macroblock `m`, as
    /// [`SliceDecoder::fill_ctx`] would put it in the grid: unavailable
    /// where `m` is `None`. A block of the picture that does not use list
    /// 0 holds a zero vector there (`resolve`, [`crate::picture::NO_MOTION`]),
    /// which is what prediction reads for it.
    #[inline(always)]
    fn l0(&self, m: Option<usize>, blk: usize) -> Nb {
        let Some(m) = m else {
            return (UNAVAILABLE, [0, 0]);
        };
        let at = m.wrapping_sub(self.motion_base);
        let band = &self.out;
        let motion = match band.heads.get(at) {
            Some(head) if !head.split => Some(&head.m),
            Some(_) => band.motion.get(at.wrapping_mul(16).wrapping_add(blk & 15)),
            None => None,
        };
        match motion {
            Some(m) => {
                debug_assert!(m.ref_idx[0] >= 0 || m.mv[0] == [0, 0]);
                (m.ref_idx[0], m.mv[0])
            }
            None => (UNAVAILABLE, [0, 0]),
        }
    }

    /// Neighbour C, or block `blk` of macroblock `d` in its place where C
    /// is not available (clause 8.4.1.3.2).
    #[inline(always)]
    fn c_or_d(&self, c: Nb, d: Option<usize>, blk: usize) -> Nb {
        if c.0 == UNAVAILABLE {
            self.l0(d, blk)
        } else {
            c
        }
    }

    /// The motion of a partition with reference index `r` and vector
    /// `mv`, as the picture keeps it: what `resolve` makes of it.
    #[inline(always)]
    fn p_motion(&self, r: i8, mv: [i16; 2]) -> Result<Motion> {
        let tables = &self.tables;
        let i = usize::try_from(r).unwrap_or(usize::MAX);
        if i >= tables.count[0] {
            return Err(Error::Invalid("a reference index past its list"));
        }
        let slot = tables.slots[0].get(i).copied().unwrap_or(NO_SLOT);
        Ok(Motion {
            mv: [mv, [0, 0]],
            ref_idx: [r, -1],
            ref_slot: [slot, NO_SLOT],
        })
    }

    /// The head of the current macroblock: its one motion, or the first
    /// partition's where `split`, whose blocks are stored.
    #[inline(always)]
    fn put_head(&mut self, m: Motion, split: bool) {
        let at = self.addr.wrapping_sub(self.motion_base);
        if let Some(head) = self.out.heads.get_mut(at) {
            *head = MbHead { m, split };
        }
        self.cur.uniform = !split;
        if cfg!(debug_assertions) {
            self.derived = 0xffff;
        }
    }

    /// The sixteen blocks of the current macroblock, row by row, each
    /// row from `rows`.
    #[inline(always)]
    fn put_blocks(&mut self, rows: [[Motion; 4]; 4]) {
        let base = self.motion_at(self.addr);
        for (y, row) in rows.iter().enumerate() {
            put_row::<4, _>(&mut self.out.motion, base + 4 * y, row);
        }
    }

    /// P_L0_16x16.
    #[inline(always)]
    fn predict_p16x16<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        let [a, b, c, d] = self.nbs;
        let r = p.ref_idx[0];
        let c = self.c_or_d(self.l0(c, 12), d, 15);
        let mvp = predict(self.l0(a, 3), self.l0(b, 12), c, r);
        let m = self.p_motion(r, add(mvp, p.mvd[0])?)?;
        self.put_head(m, false);
        self.motion_compensate(tier, super::part(0, 0, 16, 16)?, m)
    }

    /// P_L0_L0_16x8: the upper partition predicts from B where B has its
    /// reference index, the lower one from A (Equations 8-203, 8-204).
    #[inline(always)]
    fn predict_p16x8<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        let [a, b, c, d] = self.nbs;
        let [r0, r1, ..] = p.ref_idx;
        let nb = self.l0(b, 12);
        let mvp = if nb.0 == r0 {
            nb.1
        } else {
            let c = self.c_or_d(self.l0(c, 12), d, 15);
            predict(self.l0(a, 3), nb, c, r0)
        };
        let m0 = self.p_motion(r0, add(mvp, p.mvd[0])?)?;
        self.motion_compensate(tier, super::part(0, 0, 16, 8)?, m0)?;
        // The lower partition: A is A's third row, B the upper partition,
        // and C, right of the macroblock, is never available, so D, A's
        // second row, takes its place.
        let na = self.l0(a, 11);
        let mvp = if na.0 == r1 {
            na.1
        } else {
            predict(na, (r0, m0.mv[0]), self.l0(a, 7), r1)
        };
        let m1 = self.p_motion(r1, add(mvp, p.mvd[1])?)?;
        self.store_two(m0, m1, [[m0; 4], [m0; 4], [m1; 4], [m1; 4]]);
        self.motion_compensate(tier, super::part(0, 8, 16, 8)?, m1)
    }

    /// P_L0_L0_8x16: the left partition predicts from A where A has its
    /// reference index, the right one from C (Equations 8-205, 8-206).
    #[inline(always)]
    fn predict_p8x16<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        let [a, b, c, d] = self.nbs;
        let [r0, r1, ..] = p.ref_idx;
        let na = self.l0(a, 3);
        let mvp = if na.0 == r0 {
            na.1
        } else {
            // C is B's third block.
            let c = self.c_or_d(self.l0(b, 14), d, 15);
            predict(na, self.l0(b, 12), c, r0)
        };
        let m0 = self.p_motion(r0, add(mvp, p.mvd[0])?)?;
        self.motion_compensate(tier, super::part(0, 0, 8, 16)?, m0)?;
        // The right partition: A is the left one, B is B's third block,
        // and D, B's second block, takes C's place where C is not
        // available.
        let nc = self.c_or_d(self.l0(c, 12), b, 13);
        let mvp = if nc.0 == r1 {
            nc.1
        } else {
            predict((r0, m0.mv[0]), self.l0(b, 14), nc, r1)
        };
        let m1 = self.p_motion(r1, add(mvp, p.mvd[1])?)?;
        let row = [m0, m0, m1, m1];
        self.store_two(m0, m1, [row; 4]);
        self.motion_compensate(tier, super::part(8, 0, 8, 16)?, m1)
    }

    /// The motion of a macroblock of two partitions, `m0` and `m1`, laid
    /// out in `rows`: its head alone where the two agree, which then
    /// stands for its blocks, as `finish_mb` would leave it.
    #[inline(always)]
    fn store_two(&mut self, m0: Motion, m1: Motion, rows: [[Motion; 4]; 4]) {
        let split = m0 != m1;
        if split {
            self.put_blocks(rows);
        }
        self.put_head(m0, split);
    }

    /// P_8x8 or P_8x8ref0 with four 8x8 sub-macroblocks: each partition's
    /// neighbours are those of an 8x8 block at its corner of the
    /// macroblock, the earlier partitions among them, and none has a
    /// directional rule.
    #[inline(always)]
    fn predict_p8x8<T: Tier>(&mut self, tier: T, p: &PPred) -> Result<()> {
        let [a, b, c, d] = self.nbs;
        let [r0, r1, r2, r3] = p.ref_idx;
        // Upper left: C is B's third block.
        let nb2 = self.l0(b, 14);
        let mvp = predict(self.l0(a, 3), self.l0(b, 12), self.c_or_d(nb2, d, 15), r0);
        let m0 = self.p_motion(r0, add(mvp, p.mvd[0])?)?;
        self.motion_compensate(tier, super::part(0, 0, 8, 8)?, m0)?;
        // Upper right: A is the upper left, B is B's third block, C is C's
        // first bottom block, and D is B's second.
        let c1 = self.c_or_d(self.l0(c, 12), b, 13);
        let mvp = predict((r0, m0.mv[0]), nb2, c1, r1);
        let m1 = self.p_motion(r1, add(mvp, p.mvd[1])?)?;
        self.motion_compensate(tier, super::part(8, 0, 8, 8)?, m1)?;
        // Lower left: A is A's third row, B the upper left and C the upper
        // right.
        let mvp = predict(self.l0(a, 11), (r0, m0.mv[0]), (r1, m1.mv[0]), r2);
        let m2 = self.p_motion(r2, add(mvp, p.mvd[2])?)?;
        self.motion_compensate(tier, super::part(0, 8, 8, 8)?, m2)?;
        // Lower right: A is the lower left, B the upper right, and C, not
        // decoded yet, gives way to D, the upper left.
        let mvp = predict((r2, m2.mv[0]), (r1, m1.mv[0]), (r0, m0.mv[0]), r3);
        let m3 = self.p_motion(r3, add(mvp, p.mvd[3])?)?;
        let split = m0 != m1 || m0 != m2 || m0 != m3;
        if split {
            let (top, bottom) = ([m0, m0, m1, m1], [m2, m2, m3, m3]);
            self.put_blocks([top, top, bottom, bottom]);
        }
        self.put_head(m0, split);
        self.motion_compensate(tier, super::part(8, 8, 8, 8)?, m3)
    }
}
