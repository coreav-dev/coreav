//! Spatial, temporal and subblock motion-vector candidates (clause 6.5).

use crate::header::FrameHeader;
use crate::tables::{
    COUNTER_TO_CONTEXT, IDX_N_COLUMN_TO_SUBBLOCK, MODE_2_COUNTER, MV_REF_BLOCKS, NUM_8X8_HIGH,
    NUM_8X8_WIDE,
};
use crate::tile::ModeInfo;

pub(crate) type Mv = [i32; 2];

pub(crate) struct Candidates {
    pub vectors: [Mv; 2],
    pub context: usize,
}

pub(crate) struct Search<'a> {
    pub modes: &'a [ModeInfo],
    pub previous: Option<&'a [ModeInfo]>,
    pub mi_rows: u32,
    pub mi_cols: u32,
    pub col_start: u32,
    pub col_end: u32,
    pub header: &'a FrameHeader,
}

#[derive(Default)]
struct List {
    vectors: [Mv; 2],
    count: usize,
}

impl List {
    fn add(&mut self, mv: Mv) {
        if self.count > 0 && self.vectors.first() == Some(&mv) {
            return;
        }
        if let Some(slot) = self.vectors.get_mut(self.count) {
            *slot = mv;
            self.count += 1;
        }
    }

    fn same(&mut self, mode: &ModeInfo, reference: i8, subblock: usize) {
        for (&frame, mvs) in mode.ref_frames.iter().zip(&mode.sub_mvs) {
            if frame == reference {
                if let Some(&mv) = mvs.get(subblock) {
                    self.add(mv);
                }
                return;
            }
        }
    }

    fn different(&mut self, mode: &ModeInfo, reference: i8, biases: &[bool; 4]) {
        let [first, second] = mode
            .sub_mvs
            .map(|mvs| mvs.last().copied().unwrap_or_default());
        for (index, (&frame, mv)) in mode.ref_frames.iter().zip([first, second]).enumerate() {
            if frame <= 0 || frame == reference || (index == 1 && first == second) {
                continue;
            }
            let bias = usize::try_from(frame).ok().and_then(|i| biases.get(i));
            let target = usize::try_from(reference).ok().and_then(|i| biases.get(i));
            if let (Some(bias), Some(target)) = (bias, target) {
                self.add(if bias == target {
                    mv
                } else {
                    mv.map(i32::saturating_neg)
                });
            }
        }
    }
}

impl Search<'_> {
    pub(crate) fn candidates(
        &self,
        row: u32,
        col: u32,
        size: u8,
        reference: i8,
        subblock: Option<usize>,
    ) -> Candidates {
        let mut list = List::default();
        let mut counter = 0usize;
        if let Some(offsets) = MV_REF_BLOCKS.get(usize::from(size)) {
            for (index, &[dr, dc]) in offsets.iter().enumerate() {
                let r = i64::from(row) + i64::from(dr);
                let c = i64::from(col) + i64::from(dc);
                let Some(mode) = self.spatial(r, c) else {
                    continue;
                };
                let mut slot = 3;
                if index < 2 {
                    counter += MODE_2_COUNTER
                        .get(usize::from(mode.mode))
                        .copied()
                        .map_or(0, usize::from);
                    slot = subblock
                        .and_then(|block| IDX_N_COLUMN_TO_SUBBLOCK.get(block))
                        .and_then(|indices| indices.get(usize::from(dc == 0)))
                        .copied()
                        .map_or(3, usize::from);
                }
                list.same(mode, reference, slot);
            }
            let previous = self
                .previous
                .and_then(|modes| self.at(modes, row.into(), col.into()));
            if let Some(mode) = previous {
                list.same(mode, reference, 3);
            }
            for &[dr, dc] in offsets {
                if let Some(mode) = self.spatial(
                    i64::from(row) + i64::from(dr),
                    i64::from(col) + i64::from(dc),
                ) {
                    list.different(mode, reference, &self.header.ref_frame_sign_bias);
                }
            }
            if let Some(mode) = previous {
                list.different(mode, reference, &self.header.ref_frame_sign_bias);
            }
        }
        Candidates {
            vectors: list.vectors.map(|mv| self.clamp(row, col, size, mv, 128)),
            context: COUNTER_TO_CONTEXT
                .get(counter)
                .copied()
                .map_or(0, usize::from),
        }
    }

    pub(crate) fn best(&self, row: u32, col: u32, size: u8, vectors: [Mv; 2]) -> [Mv; 2] {
        vectors.map(|mut mv| {
            if !self.header.allow_high_precision_mv || !use_hp(mv) {
                mv = mv.map(|v| v - v % 2);
            }
            self.clamp(row, col, size, mv, (160 - 4) << 3)
        })
    }

    fn at<'a>(&self, modes: &'a [ModeInfo], row: i64, col: i64) -> Option<&'a ModeInfo> {
        if row < 0 || row >= i64::from(self.mi_rows) || col < 0 || col >= i64::from(self.mi_cols) {
            return None;
        }
        let index = u64::try_from(row).ok()? * u64::from(self.mi_cols) + u64::try_from(col).ok()?;
        modes.get(usize::try_from(index).ok()?)
    }

    fn spatial(&self, row: i64, col: i64) -> Option<&ModeInfo> {
        if col < i64::from(self.col_start) || col >= i64::from(self.col_end) {
            return None;
        }
        self.at(self.modes, row, col)
    }

    fn clamp(&self, row: u32, col: u32, size: u8, mv: Mv, border: i64) -> Mv {
        let height = NUM_8X8_HIGH.get(usize::from(size)).copied().unwrap_or(1);
        let width = NUM_8X8_WIDE.get(usize::from(size)).copied().unwrap_or(1);
        let [r, c] = mv;
        let component = |value: i32, position: u32, total: u32, extent: u8| {
            let min = -i64::from(position) * 64 - border;
            let max = (i64::from(total) - i64::from(extent) - i64::from(position)) * 64 + border;
            i64::from(value)
                .clamp(min, max.max(min))
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        };
        [
            component(r, row, self.mi_rows, height),
            component(c, col, self.mi_cols, width),
        ]
    }
}

pub(crate) fn use_hp(mv: Mv) -> bool {
    mv.into_iter().all(|v| (v.unsigned_abs() >> 3) < 8)
}

pub(crate) fn append_sub8x8(raw: [Mv; 2], earlier: [Mv; 4], block: usize) -> [Mv; 2] {
    if block == 0 {
        return raw;
    }
    let [first, second, third, _] = earlier;
    let mut list = List::default();
    if block <= 2 {
        list.add(first);
    } else {
        list.add(third);
        list.add(second);
        list.add(first);
    }
    for mv in raw {
        list.add(mv);
    }
    list.vectors
}

#[cfg(test)]
mod tests;
