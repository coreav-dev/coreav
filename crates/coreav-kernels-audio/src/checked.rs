//! The proof that a filter's windows lie inside their slices.

use crate::LANES;

/// Windows checked against the input and the table: only [`Checked::new`]
/// makes one, after its check, and its fields are private to this module,
/// so holding one is the proof the vector kernel's loads rely on.
#[derive(Clone, Copy)]
pub(crate) struct Checked<'a> {
    input: &'a [f32],
    table: &'a [f32],
    taps: usize,
    windows: &'a [(usize, usize)],
}

impl<'a> Checked<'a> {
    /// The windows of [`crate::Fir::run`], when `taps` is a positive
    /// multiple of [`LANES`], `outputs` is the number of windows and every
    /// window `(start, row)` has `start + taps <= input.len()` and `row +
    /// taps <= table.len()` (no windows pass whatever the slices).
    pub(crate) fn new(
        input: &'a [f32],
        table: &'a [f32],
        taps: usize,
        windows: &'a [(usize, usize)],
        outputs: usize,
    ) -> Option<Self> {
        if taps == 0 || taps % LANES != 0 || outputs != windows.len() {
            return None;
        }
        let checked = Self {
            input,
            table,
            taps,
            windows,
        };
        if windows.is_empty() {
            return Some(checked);
        }
        // One branch-free pass: every window starts at most `taps` before
        // the end of its slice.
        let last_start = input.len().checked_sub(taps)?;
        let last_row = table.len().checked_sub(taps)?;
        let inside = windows.iter().fold(true, |ok, &(start, row)| {
            ok & (start <= last_start) & (row <= last_row)
        });
        inside.then_some(checked)
    }

    pub(crate) fn input(&self) -> &'a [f32] {
        self.input
    }

    pub(crate) fn table(&self) -> &'a [f32] {
        self.table
    }

    /// A positive multiple of [`LANES`].
    pub(crate) fn taps(&self) -> usize {
        self.taps
    }

    /// Each inside its slices, as many as the outputs.
    pub(crate) fn windows(&self) -> &'a [(usize, usize)] {
        self.windows
    }
}
