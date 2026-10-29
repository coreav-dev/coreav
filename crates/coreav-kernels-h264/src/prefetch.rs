//! Prefetch hints for the decoder's picture rows: a request that the cache
//! fetch a row before a kernel reads it, or before the decoder writes it.
//!
//! The motion-compensated prediction spends much of its time waiting for
//! rows of the reference picture to arrive: its hottest instruction is the
//! one after the load of a reference row. A hint issued some way ahead of
//! the read overlaps that wait with other work. Where and how far ahead to
//! hint is the decoder's choice; these functions only issue the hints, one
//! a row, so `rows` bounds their work (a stride of 0 hints one line `rows`
//! times).
//!
//! A hint makes no architectural access: it cannot fault and changes no
//! value, so a wrong one costs time, never correctness. Still, each is
//! issued only for an index inside the slice, so no hint names memory the
//! caller does not hold. On x86-64 both hints are `prefetcht0` (the two
//! functions say what the caller intends, so that a write hint can change
//! alone later); elsewhere both are no-ops for now. [`Ownership`] issues
//! `prefetchw`, which asks for the line to be written by this core, on a
//! CPU that has it.

/// Hints that each of `rows` rows of `plane`, the first at index `first`
/// and each `stride` further, will be read: one hint per row, at the row's
/// first sample. Rows from the first one that lies past the slice (or
/// whose index overflows) on are not hinted.
#[inline]
pub fn rows_for_read(plane: &[u8], first: usize, stride: usize, rows: usize) {
    for_rows(plane, first, stride, rows, crate::simd::hint_read);
}

/// [`rows_for_read`] for rows that will be written.
#[inline]
pub fn rows_for_write(plane: &[u8], first: usize, stride: usize, rows: usize) {
    for_rows(plane, first, stride, rows, crate::simd::hint_write);
}

/// The right to issue ownership hints, which only a CPU with `prefetchw`
/// grants ([`Ownership::detect`]). An ownership hint fetches a line for
/// writing: where the line sits shared in other cores' caches, as a
/// reused buffer's lines do after those cores read them, a read hint
/// fetches a shared copy and the store still has to take the line from
/// them, while an ownership hint takes it once (`docs/design/h264.md`,
/// "Threading round: a banded picture's lines asked for ahead").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ownership(());

impl Ownership {
    /// The right, where this CPU has `prefetchw`; none elsewhere, including
    /// every CPU that is not x86-64.
    pub fn detect() -> Option<Ownership> {
        crate::simd::has_prefetchw().then_some(Ownership(()))
    }

    /// Hints that each of `rows` rows of `plane`, the first at index
    /// `first` and each `stride` further, will be written, and asks for
    /// each line to be owned: one hint per row, as [`rows_for_write`].
    #[inline]
    pub fn rows(self, plane: &[u8], first: usize, stride: usize, rows: usize) {
        for_rows(plane, first, stride, rows, |at| {
            crate::simd::hint_own(self, at)
        });
    }
}

/// One check of where the last row starts. When it lies inside the plane,
/// so does every row before it, and the counts the decoder asks for (16,
/// 8, 4 and 2 rows) are hinted straight from the first row's address with
/// no check a row: about one and a half instructions a hint, where the
/// strided walk below takes seven (`docs/design/h264.md`, "The reference
/// hints against FFmpeg's prefetch: stall or issue"). Other counts, and
/// rows that run past the plane, take the walk: one check of where the
/// rows start, then a stride at a time until the rows or the slice end.
#[inline(always)]
fn for_rows(
    plane: &[u8],
    first: usize,
    stride: usize,
    rows: usize,
    mut hint: impl FnMut(*const u8),
) {
    let last = rows
        .checked_sub(1)
        .and_then(|r| r.checked_mul(stride))
        .and_then(|offset| offset.checked_add(first));
    if last.is_some_and(|last| last < plane.len()) {
        let at = plane.as_ptr().wrapping_add(first);
        match rows {
            16 => return straight::<16>(at, stride, hint),
            8 => return straight::<8>(at, stride, hint),
            4 => return straight::<4>(at, stride, hint),
            2 => return straight::<2>(at, stride, hint),
            _ => {}
        }
    }
    let Some(from) = plane.get(first..) else {
        return;
    };
    if stride == 0 {
        if let Some(sample) = from.first() {
            (0..rows).for_each(|_| hint(core::ptr::from_ref(sample)));
        }
        return;
    }
    from.iter()
        .step_by(stride)
        .take(rows)
        .for_each(|sample| hint(core::ptr::from_ref(sample)));
}

/// `N` rows from `at`, `stride` apart, all of which [`for_rows`] has found
/// inside the plane (so `(N - 1) * stride` does not overflow).
#[inline(always)]
fn straight<const N: usize>(at: *const u8, stride: usize, mut hint: impl FnMut(*const u8)) {
    for row in 0..N {
        hint(at.wrapping_add(row.wrapping_mul(stride)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_stop_at_the_end_of_the_plane_and_change_nothing() {
        let plane: Vec<u8> = (0..=255).collect();
        let before = plane.clone();
        for (first, stride, rows) in [
            (0, 16, 16),
            (0, 16, 17),
            (255, 1, 4),
            (256, 16, 2),
            (usize::MAX, 1, 2),
            (8, usize::MAX, 3),
            (0, 0, 5),
            (0, 16, 0),
        ] {
            rows_for_read(&plane, first, stride, rows);
            rows_for_write(&plane, first, stride, rows);
        }
        rows_for_read(&[], 0, 1, 1);
        if let Some(own) = Ownership::detect() {
            for (first, stride, rows) in [(0, 16, 17), (255, 1, 4), (usize::MAX, 1, 2), (0, 0, 5)] {
                own.rows(&plane, first, stride, rows);
            }
            own.rows(&[], 0, 1, 1);
        }
        assert_eq!(plane, before);
    }

    #[test]
    fn the_rows_hinted_are_those_inside_the_plane() {
        // The rows a checked index a row would hint: from `first`, `stride`
        // apart, until one lies past the plane or its index overflows.
        fn expected(len: usize, first: usize, stride: usize, rows: usize) -> Vec<usize> {
            let mut out = Vec::new();
            let mut at = Some(first);
            for _ in 0..rows {
                match at {
                    Some(i) if i < len => out.push(i),
                    _ => break,
                }
                at = at.and_then(|i| i.checked_add(stride));
            }
            out
        }
        let plane = [0u8; 100];
        for first in [0, 1, 4, 37, 84, 85, 99, 100, 101, usize::MAX] {
            for stride in [0, 1, 5, 6, 7, 12, 33, 50, 99, 100, usize::MAX] {
                for rows in [0, 1, 2, 3, 4, 8, 15, 16, 200] {
                    let mut seen = Vec::new();
                    for_rows(&plane, first, stride, rows, |s: *const u8| {
                        seen.push(s.addr() - plane.as_ptr().addr());
                    });
                    assert_eq!(
                        seen,
                        expected(plane.len(), first, stride, rows),
                        "{first} {stride} {rows}"
                    );
                }
            }
        }
    }
}
