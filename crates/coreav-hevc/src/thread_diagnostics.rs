//! Untimed diagnostic counters and bounded read traces, absent from ordinary builds.
//! Diagnostic interfaces have no stability guarantee.

use std::cell::RefCell;

mod rows;
pub(crate) use rows::{
    ReadContext, ReadGuard, check_row_threads, motion_read, row_picture, sample_read,
};
pub use rows::{RowDependency, RowPicture, RowReference, begin_rows, take_rows};

#[doc(hidden)]
pub struct DependencySnapshot {
    pub rps: [Option<(u64, i32, bool)>; 16],
    pub active: [Option<(u64, i32, bool)>; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct DependencyOutput {
    pub event: crate::OutputEvent,
    pub before_picture: u64,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct DependencyOutputs {
    pub events: [Option<DependencyOutput>; 32],
    pub finishing: bool,
    pub current_picture: Option<u64>,
    pub deferred: bool,
}

thread_local! {
    static OUTPUTS: RefCell<Option<DependencyOutputs>> = const { RefCell::new(None) };
}

pub(crate) fn capture_outputs<T>(outputs: &mut DependencyOutputs, run: impl FnOnce() -> T) -> T {
    struct Capture<'a> {
        target: &'a mut DependencyOutputs,
        previous: Option<DependencyOutputs>,
    }
    impl Drop for Capture<'_> {
        fn drop(&mut self) {
            if let Some(completed) = OUTPUTS.replace(self.previous.take()) {
                *self.target = completed;
            }
        }
    }
    let previous = OUTPUTS.replace(Some(*outputs));
    let _capture = Capture {
        target: outputs,
        previous,
    };
    run()
}

pub(crate) fn output_picture(index: Option<u64>) {
    OUTPUTS.with_borrow_mut(|outputs| {
        if let Some(outputs) = outputs {
            outputs.current_picture = index;
            outputs.deferred = false;
        }
    });
}

pub(crate) fn output_finish() {
    OUTPUTS.with_borrow_mut(|outputs| {
        if let Some(outputs) = outputs {
            outputs.finishing = true;
        }
    });
}

pub(crate) fn output_event(
    event: crate::OutputEvent,
    count: u64,
    current_stored: bool,
) -> crate::Result<()> {
    OUTPUTS.with_borrow_mut(|outputs| {
        let Some(outputs) = outputs else {
            return Ok(());
        };
        let before_picture = match outputs.current_picture {
            Some(current) if outputs.deferred || event.picture_index == current => count,
            Some(_) => count
                .checked_sub(1)
                .ok_or(crate::Error::Internal("dependency_output_order"))?,
            None => count
                .checked_sub(u64::from(!outputs.finishing && !current_stored))
                .ok_or(crate::Error::Internal("dependency_output_order"))?,
        };
        *outputs
            .events
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(crate::Error::Limit("dependency_output_count"))? = Some(DependencyOutput {
            event,
            before_picture,
        });
        if outputs.current_picture == Some(event.picture_index) {
            outputs.deferred = true;
        }
        Ok(())
    })
}

const LABELS: [&str; 35] = [
    "barrier_workers_disabled",
    "barrier_wpp",
    "barrier_output_random_access",
    "barrier_output_capacity",
    "barrier_output_reorder",
    "barrier_output_latency",
    "barrier_reference_unavailable",
    "barrier_memory_input",
    "barrier_memory_scratch",
    "barrier_memory_frame",
    "barrier_queue_budget",
    "barrier_queue_memory",
    "barrier_snapshot_allocation",
    "barrier_window_full",
    "barrier_duplicate_hash",
    "barrier_worker_spawn",
    "event_picture_i",
    "event_picture_p",
    "event_picture_b",
    "event_prefix_sei",
    "event_suffix_sei_informational",
    "event_suffix_sei_hash",
    "event_parameter_set",
    "event_queue_admitted",
    "event_spawned_job",
    "event_caller_job",
    "event_forced_flush",
    "event_reconcile",
    "event_sequence_end",
    "event_sei_type_0",
    "event_sei_type_1",
    "event_sei_type_5",
    "event_sei_type_129",
    "event_sei_type_132",
    "event_sei_type_other",
];

pub(crate) enum Barrier {
    WorkersDisabled,
    Wpp,
    OutputRandomAccess,
    OutputCapacity,
    OutputReorder,
    OutputLatency,
    ReferenceUnavailable,
    MemoryInput,
    MemoryScratch,
    MemoryFrame,
    QueueBudget,
    QueueMemory,
    SnapshotAllocation,
    WindowFull,
    DuplicateHash,
    WorkerSpawn,
}

pub(crate) enum Event {
    PictureI,
    PictureP,
    PictureB,
    PrefixSei,
    SuffixSeiInformational,
    SuffixSeiHash,
    ParameterSet,
    QueueAdmitted,
    SpawnedJob,
    CallerJob,
    ForcedFlush,
    Reconcile,
    SequenceEnd,
    SeiType0,
    SeiType1,
    SeiType5,
    SeiType129,
    SeiType132,
    SeiTypeOther,
}

struct Counters {
    picture: Option<u64>,
    seen: u32,
    counts: [u64; LABELS.len()],
}

impl Default for Counters {
    fn default() -> Self {
        Self {
            picture: None,
            seen: 0,
            counts: [0; LABELS.len()],
        }
    }
}

thread_local! {
    static COUNTERS: RefCell<Counters> = RefCell::new(Counters::default());
}

pub(crate) fn picture(index: u64) {
    COUNTERS.with_borrow_mut(|counters| {
        if counters.picture != Some(index) {
            counters.picture = Some(index);
            counters.seen = 0;
        }
    });
}

pub(crate) fn barrier(reason: Barrier) {
    COUNTERS.with_borrow_mut(|counters| {
        let index = reason as usize;
        let mask = 1 << index;
        if counters.picture.is_none() || counters.seen & mask != 0 {
            return;
        }
        counters.seen |= mask;
        if let Some(count) = counters.counts.get_mut(index) {
            *count = count.saturating_add(1);
        }
    });
}

pub(crate) fn event(reason: Event) {
    COUNTERS.with_borrow_mut(|counters| {
        if let Some(count) = counters.counts.get_mut(16 + reason as usize) {
            *count = count.saturating_add(1);
        }
    });
}

/// Takes and resets profiling counters; barrier reasons may overlap per picture.
pub fn take() -> [(&'static str, u64); 35] {
    COUNTERS.with_borrow_mut(|counters| {
        let mut counts = std::mem::take(counters).counts.into_iter();
        LABELS.map(|label| (label, counts.next().unwrap_or(0)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_event_refuses_an_impossible_zero_count() {
        let event = crate::OutputEvent {
            poc: 0,
            picture_index: 0,
            width: 8,
            height: 8,
            bit_depth: 8,
        };
        for current_picture in [None, Some(1)] {
            let mut outputs = DependencyOutputs {
                current_picture,
                ..DependencyOutputs::default()
            };
            let result = capture_outputs(&mut outputs, || output_event(event, 0, false));
            assert_eq!(
                result,
                Err(crate::Error::Internal("dependency_output_order"))
            );
            assert!(outputs.events.iter().all(Option::is_none));
        }
    }

    #[test]
    fn barriers_are_per_picture_while_events_count_every_operation() {
        take();
        barrier(Barrier::QueueMemory);
        picture(2);
        barrier(Barrier::QueueMemory);
        barrier(Barrier::QueueMemory);
        event(Event::QueueAdmitted);
        event(Event::QueueAdmitted);
        picture(3);
        barrier(Barrier::QueueMemory);
        let values = take();
        assert_eq!(
            values
                .iter()
                .filter(|(name, _)| *name == "barrier_queue_memory")
                .map(|(_, count)| *count)
                .sum::<u64>(),
            2
        );
        assert_eq!(
            values
                .iter()
                .filter(|(name, _)| *name == "event_queue_admitted")
                .map(|(_, count)| *count)
                .sum::<u64>(),
            2
        );
        assert!(take().into_iter().all(|(_, count)| count == 0));
    }
}
