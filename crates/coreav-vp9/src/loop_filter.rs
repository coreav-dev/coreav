//! The loop filter (clause 8.8) of intra frames: the filter level of each
//! 8x8 block, and which samples across which edges of its transform and
//! prediction blocks are filtered, in the order of clause 8.8.2. The
//! filters are the kernels crate's.

use coreav_kernels_vp9::{BitDepth, Edge, FilterSize, Strength, filter_edge};

use crate::error::{Error, Result};
use crate::header::{FrameHeader, LoopFilter, MAX_SEGMENTS, Segmentation};
use crate::picture::{Planes, Sample};
use crate::tables::{NUM_8X8_HIGH, NUM_8X8_WIDE};
use crate::tile::{ModeInfo, uv_tx_size};

const MAX_LOOP_FILTER: i32 = 63;
const MAX_REF_FRAMES: usize = 4;
const MAX_MODE_LF_DELTAS: usize = 2;
const SEG_LVL_ALT_L: usize = 1;
const BLOCK_16X16: u8 = 6;
const TX_4X4: u8 = 0;
const TX_8X8: u8 = 1;
const TX_16X16: u8 = 2;

/// `LvlLookup`: the filter level of each segment, reference frame and mode
/// type.
type Levels = [[[u8; MAX_MODE_LF_DELTAS]; MAX_REF_FRAMES]; MAX_SEGMENTS];

/// The loop filter frame init process (clause 8.8.1).
fn levels(lf: &LoopFilter, seg: &Segmentation) -> Levels {
    let level = i32::from(lf.level);
    let n_shift = lf.level >> 5;
    core::array::from_fn(|segment| {
        let alt = seg
            .features
            .get(segment)
            .and_then(|f| f.get(SEG_LVL_ALT_L))
            .copied()
            .flatten()
            .filter(|_| seg.enabled);
        let lvl_seg = alt.map_or(level, |data| {
            let data = i32::from(data);
            let lvl = if seg.abs_or_delta_update {
                data
            } else {
                data + level
            };
            lvl.clamp(0, MAX_LOOP_FILTER)
        });
        core::array::from_fn(|reference| {
            core::array::from_fn(|mode| {
                if !lf.delta_enabled {
                    return lvl_seg as u8;
                }
                let delta =
                    |deltas: &[i8], i: usize| deltas.get(i).map_or(0, |&d| i32::from(d) << n_shift);
                let mode_delta = if reference == 0 {
                    0
                } else {
                    delta(&lf.mode_deltas, mode)
                };
                (lvl_seg + delta(&lf.ref_deltas, reference) + mode_delta).clamp(0, MAX_LOOP_FILTER)
                    as u8
            })
        })
    })
}

/// The loop filter process (clause 8.8) over `frame`, an intra frame whose
/// 8x8 blocks have the mode info `mi` in raster order.
pub(crate) fn apply<P: Sample>(
    h: &FrameHeader,
    mi: &[ModeInfo],
    frame: &mut Planes<P>,
) -> Result<()> {
    if h.loop_filter.level == 0 {
        return Ok(());
    }
    let levels = levels(&h.loop_filter, &h.segmentation);
    let strengths: [Option<Strength>; 64] =
        core::array::from_fn(|lvl| Strength::new(lvl as u8, h.loop_filter.sharpness));
    let bit_depth = BitDepth::from_bits(h.color.bit_depth)
        .ok_or(Error::Unsupported("a bit depth other than 8, 10 or 12"))?;
    let ss = (h.color.subsampling_x, h.color.subsampling_y);
    let (mi_rows, mi_cols) = (h.mi_rows(), h.mi_cols());
    for row in (0..mi_rows).step_by(8) {
        for col in (0..mi_cols).step_by(8) {
            for plane in 0..3 {
                let (sub_x, sub_y) = if plane == 0 {
                    (0, 0)
                } else {
                    (u32::from(ss.0), u32::from(ss.1))
                };
                let Some(samples) = frame.planes.get_mut(plane) else {
                    continue;
                };
                for vertical in [true, false] {
                    let (sub, edge_len) = if vertical {
                        (sub_x, 64 >> sub_y)
                    } else {
                        (sub_y, 64 >> sub_x)
                    };
                    for edge in 0..(16 >> sub) {
                        for i in 0..edge_len {
                            let (x, y) = if vertical {
                                (col * 8 + edge * (4 << sub_x), row * 8 + (i << sub_y))
                            } else {
                                (col * 8 + (i << sub_x), row * 8 + edge * (4 << sub_y))
                            };
                            let on_screen = x < 8 * mi_cols
                                && y < 8 * mi_rows
                                && if vertical { x > 0 } else { y > 0 };
                            if !on_screen {
                                continue;
                            }
                            let loop_col = ((x >> 3) >> sub_x) << sub_x;
                            let loop_row = ((y >> 3) >> sub_y) << sub_y;
                            let m = mi
                                .get(loop_row as usize * mi_cols as usize + loop_col as usize)
                                .copied()
                                .unwrap_or_default();
                            let tx_size = if plane == 0 {
                                m.tx_size
                            } else {
                                uv_tx_size(m.size, m.tx_size, ss)?
                            };
                            let sb_size = if sub == 0 {
                                m.size
                            } else {
                                m.size.max(BLOCK_16X16)
                            };
                            let (blocks, at) = if vertical {
                                (NUM_8X8_WIDE.get(usize::from(sb_size)), x)
                            } else {
                                (NUM_8X8_HIGH.get(usize::from(sb_size)), y)
                            };
                            let block_edge =
                                at % (8 * u32::from(blocks.copied().unwrap_or(1))) == 0;
                            let chroma_past_the_right = !vertical
                                && sub_x == 1
                                && mi_cols % 2 == 1
                                && edge % 2 == 1
                                && x + 8 >= mi_cols * 8;
                            let tx_edge = !chroma_past_the_right && edge % (1 << tx_size) == 0;
                            if !block_edge && !(tx_edge && (m.ref_frames[0] <= 0 || !m.skip)) {
                                continue;
                            }
                            let base = if tx_size == TX_4X4 && edge % 8 == 0 {
                                TX_8X8
                            } else {
                                tx_size.min(TX_16X16)
                            };
                            let at_last_chroma = if vertical {
                                sub_x == 1 && x >> 3 == mi_cols - 1
                            } else {
                                sub_y == 1 && y >> 3 == mi_rows - 1
                            };
                            let size = match base {
                                TX_4X4 => FilterSize::Four,
                                TX_16X16 if !at_last_chroma => FilterSize::Sixteen,
                                _ => FilterSize::Eight,
                            };
                            let lvl = levels
                                .get(usize::from(m.segment_id))
                                .and_then(|by_ref| by_ref.get(m.ref_frames[0].max(0) as usize))
                                .and_then(|by_mode| {
                                    by_mode.get(usize::from(matches!(m.mode, 10 | 11 | 13)))
                                })
                                .copied()
                                .unwrap_or(0);
                            let Some(strength) = strengths.get(usize::from(lvl)).copied().flatten()
                            else {
                                continue;
                            };
                            let at = (y >> sub_y) as usize * samples.stride + (x >> sub_x) as usize;
                            let step = if vertical { 1 } else { samples.stride };
                            let edge = Edge::new(&mut samples.samples, at, step, size)
                                .ok_or(Error::Invalid("a loop filter edge outside the frame"))?;
                            filter_edge(edge, strength, bit_depth);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
