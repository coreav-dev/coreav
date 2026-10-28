//! Decode-order lookups in a track's sample tables without listing its
//! samples (`docs/design/seek.md`): which sample a decode time falls in, the
//! sync sample at or before a sample, and the range of composition offsets.
//! Together with [`Movie::packets_from`] they let a caller read the packets
//! around a time instead of every packet before it.

use crate::Error;
use crate::movie::{Movie, Track};
use crate::packets::Budget;
use crate::tables::Table;

/// One `stts` entry with samples, with where it starts.
#[derive(Clone, Copy, Debug)]
struct Step {
    /// First sample of the entry (0-based).
    first: u32,
    /// Decode time of that sample.
    dts: i64,
    /// Decode duration of each of its samples.
    delta: u32,
}

/// Lookups in the sample tables of one track of a movie (its movie box
/// samples; movie fragments are not covered).
#[derive(Clone, Debug)]
pub struct Seeker<'a> {
    track: &'a Track,
    samples: u32,
    steps: Vec<Step>,
    offsets: (i64, i64),
}

impl Movie {
    /// Lookups in track `index`'s sample tables, `None` when it has none
    /// that iteration would read (external data, invalid tables). Building
    /// them reads each `stts` and `ctts` entry once, charged to `budget`.
    pub fn seeker(&self, index: usize, budget: &mut Budget) -> Result<Option<Seeker<'_>>, Error> {
        let track = self.tracks.get(index).ok_or(Error::NoSuchTrack)?;
        if !track.tables_valid || track.external_data {
            return Ok(None);
        }
        let tables = &track.tables;
        let Some(stts) = &tables.stts else {
            return Ok(None);
        };
        let mut steps = Vec::new();
        let (mut first, mut dts) = (0u32, 0i64);
        for entry in 0..stts.count {
            budget.charge(1)?;
            let (Some(count), Some(delta)) = (stts.field(entry, 0, 8), stts.field(entry, 4, 8))
            else {
                return Ok(None);
            };
            if count > 0 {
                steps.push(Step { first, dts, delta });
            }
            first = first.saturating_add(count);
            // Validation bounds the total duration within i64.
            dts = i64::from(count)
                .checked_mul(i64::from(delta))
                .and_then(|d| dts.checked_add(d))
                .ok_or(Error::Overflow { offset: 0 })?;
        }
        let offsets = match &tables.ctts {
            Some(ctts) => match offset_range(ctts, budget)? {
                Some(range) => range,
                None => return Ok(None),
            },
            None => (0, 0),
        };
        Ok(Some(Seeker {
            track,
            samples: tables.sample_count(),
            steps,
            offsets,
        }))
    }
}

/// The least and the greatest composition offset of `ctts`, as iteration
/// reads them (signed or not as the table says).
fn offset_range(ctts: &Table, budget: &mut Budget) -> Result<Option<(i64, i64)>, Error> {
    let mut range: Option<(i64, i64)> = None;
    for entry in 0..ctts.count {
        budget.charge(1)?;
        let (Some(count), Some(raw)) = (ctts.field(entry, 0, 8), ctts.field(entry, 4, 8)) else {
            return Ok(None);
        };
        if count == 0 {
            continue;
        }
        let value = if ctts.signed {
            i64::from(raw as i32)
        } else {
            i64::from(raw)
        };
        range = Some(match range {
            Some((low, high)) => (low.min(value), high.max(value)),
            None => (value, value),
        });
    }
    Ok(Some(range.unwrap_or((0, 0))))
}

impl Seeker<'_> {
    /// Samples in the sample tables.
    pub fn sample_count(&self) -> u32 {
        self.samples
    }

    /// Decode time of sample `sample` (0-based), in media ticks; for the
    /// sample count, the end of the last sample.
    pub fn dts(&self, sample: u32) -> Option<i64> {
        if sample > self.samples {
            return None;
        }
        let at = self.steps.partition_point(|s| s.first <= sample);
        let Some(step) = at.checked_sub(1).and_then(|i| self.steps.get(i)) else {
            return Some(0);
        };
        i64::from(sample - step.first)
            .checked_mul(i64::from(step.delta))
            .and_then(|d| step.dts.checked_add(d))
    }

    /// How many samples have a decode time at or before `ticks`: the index
    /// of the first sample after it (decode times never decrease).
    pub fn samples_through(&self, ticks: i64) -> u32 {
        let at = self.steps.partition_point(|s| s.dts <= ticks);
        let Some(step) = at.checked_sub(1).and_then(|i| self.steps.get(i)) else {
            return 0;
        };
        let end = self.steps.get(at).map_or(self.samples, |next| next.first);
        let within = if step.delta == 0 {
            u64::from(end - step.first)
        } else {
            u64::try_from((ticks - step.dts) / i64::from(step.delta))
                .unwrap_or(0)
                .saturating_add(1)
        };
        u32::try_from(u64::from(step.first).saturating_add(within))
            .unwrap_or(u32::MAX)
            .min(end)
    }

    /// The last sync sample at or before `sample` (0-based), `None` when
    /// there is none. Without `stss` every sample is a sync sample.
    pub fn sync_at_or_before(&self, sample: u32) -> Option<u32> {
        let sample = sample.min(self.samples.checked_sub(1)?);
        let Some(stss) = &self.track.tables.stss else {
            return Some(sample);
        };
        // 1-based numbers, strictly increasing after validation.
        let number = sample.checked_add(1)?;
        let (mut low, mut high) = (0u32, stss.count);
        while low < high {
            let middle = low + (high - low) / 2;
            if stss.field(middle, 0, 4)? <= number {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        stss.field(low.checked_sub(1)?, 0, 4)?.checked_sub(1)
    }

    /// The first sync sample, `None` when there is none.
    pub fn first_sync(&self) -> Option<u32> {
        if self.samples == 0 {
            return None;
        }
        match &self.track.tables.stss {
            None => Some(0),
            Some(stss) => stss.field(0, 0, 4)?.checked_sub(1),
        }
    }

    /// The least and the greatest composition offset of any sample, in
    /// media ticks, as iteration reads them; (0, 0) without `ctts`.
    pub fn composition_offsets(&self) -> (i64, i64) {
        self.offsets
    }
}
