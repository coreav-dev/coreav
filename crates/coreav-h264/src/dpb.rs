//! The decoded picture buffer: reference marking (clause 8.2.5), reference
//! picture lists (clause 8.2.4) and output order (Annex C.4.5).
#![forbid(unsafe_code)]

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::picture::Picture;
use crate::slice::{ListModification, Mmco, SliceHeader};

/// How a picture of the buffer is marked (clause 8.2.5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefMark {
    /// "unused for reference".
    Unused,
    /// "used for short-term reference".
    Short,
    /// "used for long-term reference".
    Long,
}

/// One picture in the buffer.
pub(crate) struct Entry {
    pub(crate) picture: Arc<Picture>,
    pub(crate) frame_num: u32,
    pub(crate) long_term_frame_idx: u32,
    pub(crate) mark: RefMark,
    pub(crate) needed_for_output: bool,
    /// A frame inferred for a gap in frame_num (clause 8.2.5.2), which was
    /// never coded.
    pub(crate) non_existing: bool,
}

impl Entry {
    /// FrameNumWrap (clause 8.2.4.1).
    fn frame_num_wrap(&self, current: u32, max_frame_num: u32) -> i32 {
        if self.frame_num > current {
            self.frame_num as i32 - max_frame_num as i32
        } else {
            self.frame_num as i32
        }
    }
}

/// One entry of a reference picture list, as the macroblock layer uses it.
#[derive(Clone)]
pub(crate) struct RefPic {
    pub(crate) picture: Arc<Picture>,
    pub(crate) long_term: bool,
}

/// The decoded picture buffer.
pub(crate) struct Dpb {
    pub(crate) entries: Vec<Entry>,
    /// How many pictures the buffer holds, from the level limits and the
    /// caller's limits.
    pub(crate) size: usize,
    /// How many pictures may wait for output before the oldest of them is
    /// output (max_num_reorder_frames, Annex E.2.1 and C.4.5.3).
    pub(crate) max_num_reorder_frames: usize,
    /// MaxLongTermFrameIdx, or `None` for "no long-term frame indices".
    pub(crate) max_long_term_frame_idx: Option<u32>,
    /// Pictures that have left the buffer, in output order.
    pub(crate) output: Vec<Arc<Picture>>,
}

impl Dpb {
    /// An empty buffer.
    pub(crate) fn new() -> Dpb {
        Dpb {
            entries: Vec::new(),
            size: 16,
            max_num_reorder_frames: 16,
            max_long_term_frame_idx: None,
            output: Vec::new(),
        }
    }

    /// Drops the pictures that are neither referenced nor waiting for
    /// output.
    fn remove_unneeded(&mut self) {
        self.entries
            .retain(|e| e.needed_for_output || e.mark != RefMark::Unused);
    }

    /// The bumping process (Annex C.4.5.3): outputs the picture with the
    /// smallest picture order count that is still waiting.
    fn bump(&mut self) -> bool {
        let next = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.needed_for_output)
            .min_by_key(|(_, e)| e.picture.poc())
            .map(|(i, _)| i);
        let Some(i) = next else { return false };
        let Some(entry) = self.entries.get_mut(i) else {
            return false;
        };
        entry.needed_for_output = false;
        self.output.push(Arc::clone(&entry.picture));
        if entry.mark == RefMark::Unused {
            self.entries.remove(i);
        }
        true
    }

    /// Outputs everything still waiting, in output order.
    pub(crate) fn flush(&mut self) {
        while self.bump() {}
        self.remove_unneeded();
    }

    /// Drops every picture without outputting it
    /// (no_output_of_prior_pics_flag).
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Pictures that are waiting for output.
    fn num_waiting(&self) -> usize {
        self.entries.iter().filter(|e| e.needed_for_output).count()
    }

    /// Outputs pictures while more than `max_num_reorder_frames` of them
    /// wait (clause C.4.5.3). A stream that says it does not reorder gets
    /// its pictures as soon as they are decoded, instead of once the
    /// buffer fills.
    pub(crate) fn output_reordered(&mut self) {
        while self.num_waiting() > self.max_num_reorder_frames {
            if !self.bump() {
                return;
            }
        }
    }

    /// Pictures marked as references.
    fn num_ref(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.mark != RefMark::Unused)
            .count()
    }

    /// The short-term picture with the smallest FrameNumWrap.
    fn oldest_short(&self, current: u32, max_frame_num: u32) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.mark == RefMark::Short)
            .min_by_key(|(_, e)| e.frame_num_wrap(current, max_frame_num))
            .map(|(i, _)| i)
    }

    /// Unmarks the entry at `i`, if it is there.
    fn unmark(&mut self, i: usize) {
        if let Some(e) = self.entries.get_mut(i) {
            e.mark = RefMark::Unused;
        }
    }

    /// Sliding window marking (clause 8.2.5.3).
    pub(crate) fn sliding_window(
        &mut self,
        max_num_ref_frames: u32,
        current: u32,
        max_frame_num: u32,
    ) {
        let max = max_num_ref_frames.max(1) as usize;
        if self.num_ref() < max {
            return;
        }
        if let Some(i) = self.oldest_short(current, max_frame_num) {
            self.unmark(i);
        }
    }

    /// Adaptive marking (clause 8.2.5.4). Returns the LongTermFrameIdx the
    /// current picture takes, if operation 6 gives it one.
    pub(crate) fn adaptive(
        &mut self,
        mmcos: &[Mmco],
        current: u32,
        max_frame_num: u32,
    ) -> Option<u32> {
        let mut current_long = None;
        for m in mmcos {
            let pic_num_x = current as i32 - (m.difference_of_pic_nums_minus1 as i32 + 1);
            match m.op {
                // Mark a short-term picture as unused.
                1 => {
                    for e in self.entries.iter_mut() {
                        if e.mark == RefMark::Short
                            && e.frame_num_wrap(current, max_frame_num) == pic_num_x
                        {
                            e.mark = RefMark::Unused;
                        }
                    }
                }
                // Mark a long-term picture as unused.
                2 => {
                    for e in self.entries.iter_mut() {
                        if e.mark == RefMark::Long && e.long_term_frame_idx == m.long_term_pic_num {
                            e.mark = RefMark::Unused;
                        }
                    }
                }
                // Turn a short-term picture into a long-term one.
                3 => {
                    let target = self.entries.iter().position(|e| {
                        e.mark == RefMark::Short
                            && e.frame_num_wrap(current, max_frame_num) == pic_num_x
                    });
                    for (i, e) in self.entries.iter_mut().enumerate() {
                        if e.mark == RefMark::Long
                            && e.long_term_frame_idx == m.long_term_frame_idx
                            && Some(i) != target
                        {
                            e.mark = RefMark::Unused;
                        }
                    }
                    if let Some(e) = target.and_then(|i| self.entries.get_mut(i)) {
                        e.mark = RefMark::Long;
                        e.long_term_frame_idx = m.long_term_frame_idx;
                    }
                }
                // Set MaxLongTermFrameIdx.
                4 => {
                    let max = m.max_long_term_frame_idx_plus1.checked_sub(1);
                    self.max_long_term_frame_idx = max;
                    for e in self.entries.iter_mut() {
                        if e.mark == RefMark::Long
                            && max.is_none_or(|mx| e.long_term_frame_idx > mx)
                        {
                            e.mark = RefMark::Unused;
                        }
                    }
                }
                // Mark every picture as unused.
                5 => {
                    for e in self.entries.iter_mut() {
                        e.mark = RefMark::Unused;
                    }
                    self.max_long_term_frame_idx = None;
                }
                // Give the current picture a long-term index.
                6 => {
                    for e in self.entries.iter_mut() {
                        if e.mark == RefMark::Long && e.long_term_frame_idx == m.long_term_frame_idx
                        {
                            e.mark = RefMark::Unused;
                        }
                    }
                    current_long = Some(m.long_term_frame_idx);
                }
                _ => {}
            }
        }
        current_long
    }

    /// Concealment for a stream whose adaptive marking leaves more than
    /// Max(max_num_ref_frames, 1) reference frames, which clause 8.2.5.4
    /// forbids: drop short-term frames as the sliding window would, and
    /// once none is left, the long-term frame of the highest
    /// LongTermFrameIdx, as lowering MaxLongTermFrameIdx would. Without the
    /// second, a stream that gives every picture a new long-term index
    /// would keep every picture, past the buffer and the memory budget.
    pub(crate) fn enforce_max_refs(
        &mut self,
        max_num_ref_frames: u32,
        current: u32,
        max_frame_num: u32,
    ) {
        let max = max_num_ref_frames.max(1) as usize;
        while self.num_ref() > max {
            let victim = self.oldest_short(current, max_frame_num).or_else(|| {
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.mark == RefMark::Long)
                    .max_by_key(|(_, e)| e.long_term_frame_idx)
                    .map(|(i, _)| i)
            });
            let Some(i) = victim else {
                return;
            };
            self.unmark(i);
        }
    }

    /// Stores the decoded picture (Annex C.4.5.1 and C.4.5.2).
    pub(crate) fn store(&mut self, entry: Entry) {
        self.remove_unneeded();
        if entry.mark == RefMark::Unused && entry.needed_for_output {
            // A non-reference picture (C.4.5.2): it may be output at once if
            // every waiting picture follows it.
            loop {
                if self.entries.len() < self.size {
                    self.entries.push(entry);
                    return;
                }
                let first = self
                    .entries
                    .iter()
                    .filter(|e| e.needed_for_output)
                    .all(|e| entry.picture.poc() < e.picture.poc());
                if first || !self.bump() {
                    self.output.push(entry.picture);
                    return;
                }
            }
        }
        while self.entries.len() >= self.size {
            if !self.bump() {
                break;
            }
        }
        self.entries.push(entry);
    }

    /// The initial reference picture lists (clause 8.2.4.2) after
    /// modification (clause 8.2.4.3), for a frame slice.
    pub(crate) fn ref_lists(
        &self,
        h: &SliceHeader,
        current_poc: i32,
        max_frame_num: u32,
        poc_type: u32,
    ) -> Result<[Vec<RefPic>; 2]> {
        let refs: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| e.mark != RefMark::Unused)
            .collect();
        // Indices into `refs`, so that identity comparisons are index
        // comparisons.
        let short: Vec<usize> = refs
            .iter()
            .enumerate()
            .filter(|(_, e)| e.mark == RefMark::Short)
            .map(|(i, _)| i)
            .collect();
        let mut long: Vec<usize> = refs
            .iter()
            .enumerate()
            .filter(|(_, e)| e.mark == RefMark::Long)
            .map(|(i, _)| i)
            .collect();
        let at = |i: usize| refs.get(i).copied();
        long.sort_by_key(|&i| at(i).map_or(u32::MAX, |e| e.long_term_frame_idx));

        let mut initial: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
        if h.slice_type.is_b() {
            // With pic_order_cnt_type 0 a picture inferred for a frame_num
            // gap has no picture order count, so it is left out of the B
            // lists (clause 8.2.4.2.3).
            let usable: Vec<usize> = short
                .iter()
                .copied()
                .filter(|&i| poc_type != 0 || at(i).is_some_and(|e| !e.non_existing))
                .collect();
            let poc = |i: usize| at(i).map_or(0, |e| e.picture.poc());
            let mut before: Vec<usize> = usable
                .iter()
                .copied()
                .filter(|&i| poc(i) < current_poc)
                .collect();
            let mut after: Vec<usize> = usable
                .iter()
                .copied()
                .filter(|&i| poc(i) > current_poc)
                .collect();
            before.sort_by_key(|&i| std::cmp::Reverse(poc(i)));
            after.sort_by_key(|&i| poc(i));
            initial[0] = before
                .iter()
                .chain(after.iter())
                .chain(long.iter())
                .copied()
                .collect();
            initial[1] = after
                .iter()
                .chain(before.iter())
                .chain(long.iter())
                .copied()
                .collect();
            // "When the two lists are identical, the first two entries of
            // list 1 are swapped" (clause 8.2.4.2.3).
            if initial[1].len() > 1 && initial[0] == initial[1] {
                initial[1].swap(0, 1);
            }
        } else if !h.slice_type.is_intra() {
            let mut short = short;
            short.sort_by_key(|&i| {
                std::cmp::Reverse(
                    at(i).map_or(i32::MIN, |e| e.frame_num_wrap(h.frame_num, max_frame_num)),
                )
            });
            initial[0] = short.iter().chain(long.iter()).copied().collect();
        }

        let mut lists: [Vec<RefPic>; 2] = [Vec::new(), Vec::new()];
        for (l, out) in lists.iter_mut().enumerate() {
            let n = h.num_ref_idx_active.get(l).copied().unwrap_or(0) as usize;
            if n == 0 {
                continue;
            }
            let mut list: Vec<Option<usize>> = initial
                .get(l)
                .into_iter()
                .flatten()
                .copied()
                .map(Some)
                .take(n)
                .collect();
            list.resize(n, None);
            if let Some(mods) = h.list_mods.get(l).filter(|m| !m.is_empty()) {
                modify(&mut list, mods, h.frame_num, max_frame_num, &refs)?;
            }
            for slot in list {
                // A list entry no picture fills cannot be referenced, and
                // neither can the entries behind it.
                let Some(e) = slot.and_then(at) else { break };
                out.push(RefPic {
                    picture: Arc::clone(&e.picture),
                    long_term: e.mark == RefMark::Long,
                });
            }
        }
        Ok(lists)
    }
}

/// Modification of a reference picture list (clause 8.2.4.3.1), over
/// indices into `refs`.
fn modify(
    list: &mut Vec<Option<usize>>,
    mods: &[ListModification],
    current: u32,
    max_frame_num: u32,
    refs: &[&Entry],
) -> Result<()> {
    let n = list.len();
    let max_pic_num = max_frame_num as i32;
    let current_pic_num = current as i32;
    let mut pred = current_pic_num;
    for (ref_idx, m) in mods.iter().enumerate() {
        if ref_idx >= n {
            return Err(Error::Invalid("too many reference list modifications"));
        }
        let found = match m.idc {
            0 | 1 => {
                // picNumLXNoWrap and picNumLX (Equations 8-34 to 8-37).
                let d = m.value as i32 + 1;
                let mut no_wrap = if m.idc == 0 { pred - d } else { pred + d };
                if no_wrap < 0 {
                    no_wrap += max_pic_num;
                } else if no_wrap >= max_pic_num {
                    no_wrap -= max_pic_num;
                }
                pred = no_wrap;
                let pic_num = if no_wrap > current_pic_num {
                    no_wrap - max_pic_num
                } else {
                    no_wrap
                };
                refs.iter()
                    .position(|e| {
                        e.mark == RefMark::Short
                            && e.frame_num_wrap(current, max_frame_num) == pic_num
                    })
                    .ok_or(Error::Invalid(
                        "a list modification names a missing short-term picture",
                    ))?
            }
            _ => refs
                .iter()
                .position(|e| e.mark == RefMark::Long && e.long_term_frame_idx == m.value)
                .ok_or(Error::Invalid(
                    "a list modification names a missing long-term picture",
                ))?,
        };
        // Place the picture at ref_idx and drop its other occurrence
        // (Equation 8-38).
        list.insert(ref_idx, Some(found));
        let mut keep = ref_idx + 1;
        for r in ref_idx + 1..list.len() {
            let entry = list.get(r).copied().flatten();
            if entry != Some(found) {
                if let Some(slot) = list.get_mut(keep) {
                    *slot = entry;
                }
                keep += 1;
            }
        }
        list.truncate(keep);
        list.resize(n, None);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;

    #[test]
    fn long_term_frames_past_the_reference_count_are_dropped() {
        // A stream that gives every picture a new long-term index keeps no
        // more reference frames than it declares: once no short-term one
        // is left, the highest index goes.
        let mut dpb = Dpb::new();
        dpb.size = 16;
        for idx in 0..10 {
            dpb.entries.push(Entry {
                picture: Arc::new(Picture::black(1, 1, &Limits::default()).unwrap()),
                frame_num: idx,
                long_term_frame_idx: idx,
                mark: RefMark::Long,
                needed_for_output: false,
                non_existing: false,
            });
        }
        dpb.enforce_max_refs(4, 0, 16);
        let kept: Vec<u32> = dpb
            .entries
            .iter()
            .filter(|e| e.mark == RefMark::Long)
            .map(|e| e.long_term_frame_idx)
            .collect();
        assert_eq!(kept, [0, 1, 2, 3]);
    }
}
