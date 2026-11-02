//! Monotonic, pre-allocation charges for one planning operation.
//!
//! The capacity proof is specific to the distribution compiler, Rust 1.99.1
//! (`58cf8f9c35b355199727627d07ae34a4e2846d67`). In its `library/alloc/src`,
//! `vec/mod.rs::Vec::try_reserve_exact` forwards to `RawVec`; in
//! `raw_vec/mod.rs`, `grow_exact` computes `len + additional`, `finish_grow`
//! requests that array layout, and `set_ptr_and_cap` stores that capacity.
//! In `alloc.rs`, `Global::alloc_impl_runtime` and `grow_impl_runtime` return
//! slices with the requested layout size, not the allocator's usable size.
//! This proof must be revisited when that compiler changes. It bounds
//! requested backing capacity, not allocator bookkeeping or rounding.

use std::alloc::Layout;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A planning charge, capacity calculation or fallible reservation failed.
///
/// The caller maps this allocation-free value to its own structured error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanningLimit;

/// One operation's monotonic allocation budget.
///
/// Every new backing store is charged in full, including a replacement for
/// an already charged allocation. Charges are never refunded, even after
/// deallocation or a failed reservation, so they conservatively cover
/// transient storage and old/new allocation overlap.
///
/// Existing vector capacity must already belong to this account. Every
/// subsequent capacity increase must use this account too. These helpers
/// do not account for allocations inside vector elements or source readers.
/// Callers without a budget can keep their ordinary vector operations.
#[derive(Clone, Debug)]
pub struct PlanningBudget {
    state: Arc<State>,
}

#[derive(Debug)]
struct State {
    max: usize,
    used: AtomicUsize,
    failed: AtomicBool,
}

impl PartialEq for PlanningBudget {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }
}

impl Eq for PlanningBudget {}

impl PlanningBudget {
    /// Starts an empty account with an inclusive byte ceiling.
    /// Clones share the account. The one Arc control-block allocation belongs
    /// to bridge bookkeeping, not to the media-planning byte total.
    pub fn new(max: usize) -> Self {
        Self {
            state: Arc::new(State {
                max,
                used: AtomicUsize::new(0),
                failed: AtomicBool::new(false),
            }),
        }
    }

    /// Charges bytes before allocating them; failure permanently closes the account.
    ///
    /// A rejected charge leaves the previously charged total unchanged.
    pub fn charge(&self, bytes: usize) -> Result<(), PlanningLimit> {
        if self.failed() {
            return Err(PlanningLimit);
        }
        let mut current = self.used();
        loop {
            if self.failed() {
                return Err(PlanningLimit);
            }
            let next = current
                .checked_add(bytes)
                .filter(|next| *next <= self.state.max)
                .ok_or_else(|| self.fail())?;
            match self.state.used.compare_exchange(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) if self.failed() => return Err(PlanningLimit),
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }

    /// Reserves room for `additional` elements, doubling existing capacity when growing.
    ///
    /// The new capacity is the larger of twice the old capacity and
    /// `len + additional`. Its entire byte layout is charged before the
    /// fallible exact reservation. No charge is added when capacity suffices.
    pub fn reserve<T>(&self, vector: &mut Vec<T>, additional: usize) -> Result<(), PlanningLimit> {
        self.reserve_capacity(vector, additional, true)
    }

    /// Reserves exactly `len + additional` elements when existing capacity is insufficient.
    ///
    /// The entire new backing layout is charged before allocation, not only
    /// the increase. The pinned compiler proof is in this module's documentation.
    pub fn reserve_exact<T>(
        &self,
        vector: &mut Vec<T>,
        additional: usize,
    ) -> Result<(), PlanningLimit> {
        self.reserve_capacity(vector, additional, false)
    }

    /// Bytes successfully charged, including charges for failed allocations.
    pub fn used(&self) -> usize {
        self.state.used.load(Ordering::SeqCst)
    }

    /// Whether any charge, arithmetic operation or reservation has failed.
    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::SeqCst)
    }

    fn reserve_capacity<T>(
        &self,
        vector: &mut Vec<T>,
        additional: usize,
        geometric: bool,
    ) -> Result<(), PlanningLimit> {
        self.charge(0)?;
        let required = vector
            .len()
            .checked_add(additional)
            .ok_or_else(|| self.fail())?;
        if required <= vector.capacity() {
            return Ok(());
        }
        let capacity = if geometric {
            vector
                .capacity()
                .checked_mul(2)
                .ok_or_else(|| self.fail())?
                .max(required)
        } else {
            required
        };
        let layout = Layout::array::<T>(capacity).map_err(|_| self.fail())?;
        self.charge(layout.size())?;
        vector
            .try_reserve_exact(capacity - vector.len())
            .map_err(|_| self.fail())
    }

    fn fail(&self) -> PlanningLimit {
        self.state.failed.store(true, Ordering::SeqCst);
        PlanningLimit
    }
}

pub(crate) fn limit(offset: u64) -> crate::Error {
    crate::Error::LimitExceeded {
        limit: crate::LimitKind::PlanningBytes,
        offset,
    }
}

pub(crate) fn reserve<T>(
    planning: Option<&PlanningBudget>,
    vector: &mut Vec<T>,
    additional: usize,
    offset: u64,
) -> Result<(), crate::Error> {
    match planning {
        Some(planning) => planning
            .reserve(vector, additional)
            .map_err(|_| limit(offset)),
        None => vector.try_reserve(additional).map_err(|_| limit(offset)),
    }
}

pub(crate) fn reserve_exact<T>(
    planning: Option<&PlanningBudget>,
    vector: &mut Vec<T>,
    additional: usize,
    offset: u64,
) -> Result<(), crate::Error> {
    match planning {
        Some(planning) => planning
            .reserve_exact(vector, additional)
            .map_err(|_| limit(offset)),
        None => vector
            .try_reserve_exact(additional)
            .map_err(|_| limit(offset)),
    }
}

pub(crate) fn copy<T: Copy>(
    planning: Option<&PlanningBudget>,
    data: &[T],
    offset: u64,
) -> Result<Vec<T>, crate::Error> {
    let mut out = Vec::new();
    reserve_exact(planning, &mut out, data.len(), offset)?;
    out.extend_from_slice(data);
    Ok(out)
}

pub(crate) fn filled<T: Copy>(
    planning: Option<&PlanningBudget>,
    value: T,
    count: usize,
    offset: u64,
) -> Result<Vec<T>, crate::Error> {
    let mut out = Vec::new();
    reserve_exact(planning, &mut out, count, offset)?;
    out.resize(count, value);
    Ok(out)
}

pub(crate) fn lossy(
    planning: Option<&PlanningBudget>,
    mut data: &[u8],
    offset: u64,
) -> Result<String, crate::Error> {
    if planning.is_none() {
        return Ok(String::from_utf8_lossy(data).into_owned());
    }
    let mut out = Vec::new();
    loop {
        match core::str::from_utf8(data) {
            Ok(valid) => {
                reserve(planning, &mut out, valid.len(), offset)?;
                out.extend_from_slice(valid.as_bytes());
                return String::from_utf8(out).map_err(|_| limit(offset));
            }
            Err(error) => {
                let valid = data.get(..error.valid_up_to()).unwrap_or_default();
                reserve(planning, &mut out, valid.len(), offset)?;
                out.extend_from_slice(valid);
                reserve(planning, &mut out, '\u{fffd}'.len_utf8(), offset)?;
                out.extend_from_slice("\u{fffd}".as_bytes());
                match error.error_len() {
                    Some(len) => {
                        data = data
                            .get(error.valid_up_to().saturating_add(len)..)
                            .unwrap_or_default()
                    }
                    None => return String::from_utf8(out).map_err(|_| limit(offset)),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_counting_alloc::{CountingAllocator, measure};

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;

    #[test]
    fn clones_share_charges_and_sticky_failure() {
        let budget = PlanningBudget::new(3);
        let cloned = budget.clone();
        assert_eq!(budget, cloned);
        assert_ne!(budget, PlanningBudget::new(3));
        assert_eq!(cloned.charge(2), Ok(()));
        assert_eq!(budget.used(), 2);
        assert_eq!(budget.charge(2), Err(PlanningLimit));
        assert!(cloned.failed());
        assert_eq!(cloned.charge(0), Err(PlanningLimit));
    }

    #[test]
    fn concurrent_clones_cannot_lose_or_overrun_charges() {
        let budget = PlanningBudget::new(800);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let budget = budget.clone();
                scope.spawn(move || {
                    for _ in 0..100 {
                        assert_eq!(budget.charge(1), Ok(()));
                    }
                });
            }
        });
        assert_eq!(budget.used(), 800);
        assert_eq!(budget.clone().charge(1), Err(PlanningLimit));
        assert!(budget.failed());
    }

    #[test]
    fn lossy_strings_match_the_standard_conversion_and_charge_capacity() {
        for data in [
            b"hello".as_slice(),
            b"a\xffb\xf0\x90",
            b"\xf0\x90\x80\x80\xff",
        ] {
            let budget = PlanningBudget::new(1024);
            let (value, counters) =
                measure(|| lossy(Some(&budget), data, 0).expect("lossy string"));
            assert_eq!(value, String::from_utf8_lossy(data));
            assert!(budget.used() >= value.capacity());
            assert!(budget.used() as u64 >= counters.peak_bytes);
        }
    }

    #[test]
    fn bounded_probe_refuses_payload_allocation_before_it_happens() {
        let data = [
            0, 0, 0, 36, b'm', b'o', b'o', b'v', 0, 0, 0, 28, b'm', b'v', b'h', b'd', 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1,
        ];
        let allowlist = crate::Allowlist::default();
        let budget = PlanningBudget::new(0);
        let (result, counters) = measure(|| {
            crate::probe_with_planning(
                crate::SliceSource::new(&data),
                &crate::Limits::default(),
                &allowlist,
                budget.clone(),
            )
        });
        assert!(matches!(
            result,
            Err(crate::Error::LimitExceeded {
                limit: crate::LimitKind::PlanningBytes,
                ..
            })
        ));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert!(budget.failed());
        assert_eq!(budget.used(), 0);
        let budget = PlanningBudget::new(20);
        let movie = crate::probe_with_planning(
            crate::SliceSource::new(&data),
            &crate::Limits::default(),
            &allowlist,
            budget.clone(),
        )
        .expect("bounded movie");
        assert_eq!(movie.planning.as_ref(), Some(&budget));
        assert_eq!(budget.used(), 20);
        assert!(
            movie
                .presentation_spans_with_planning(&budget)
                .expect("spans")
                .is_empty()
        );
        assert_eq!(budget.charge(1), Err(PlanningLimit));
        assert!(matches!(
            movie.seeker(0, &mut crate::Budget::new(&crate::Limits::default())),
            Err(crate::Error::LimitExceeded {
                limit: crate::LimitKind::PlanningBytes,
                ..
            })
        ));
    }

    #[test]
    fn parameter_set_descriptors_charge_full_growth_without_copying_payloads() {
        let record = [1, 66, 0, 10, 255, 225, 0, 2, 0x67, 0, 1, 0, 2, 0x68, 0];
        let budget = PlanningBudget::new(3 * size_of::<&[u8]>());
        let sets = crate::avc_parameter_sets_with_planning(&record, Some(&budget))
            .expect("account")
            .expect("record");
        assert_eq!(sets.len(), 2);
        assert_eq!(budget.used(), 3 * size_of::<&[u8]>());
        let budget = PlanningBudget::new(0);
        let (result, counters) =
            measure(|| crate::avc_parameter_sets_with_planning(&record, Some(&budget)));
        assert_eq!(result, Err(PlanningLimit));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
    }

    #[test]
    fn shared_configuration_and_access_scratch_allocate_nothing() {
        let record = [1, 66, 0, 10, 255, 225, 0, 2, 0x67, 0, 1, 0, 2, 0x68, 0];
        let budget = PlanningBudget::new(0);
        let ((), counters) = measure(|| {
            assert_eq!(
                crate::describe_config_record_with_planning(
                    crate::FourCc(*b"avcC"),
                    &record,
                    Some(&budget)
                ),
                Ok(crate::describe_config_record(
                    crate::FourCc(*b"avcC"),
                    &record
                ))
            );
            assert_eq!(
                crate::parameter_set_color_with_planning(
                    crate::FourCc(*b"avcC"),
                    &record,
                    Some(&budget)
                ),
                Ok(crate::parameter_set_color(crate::FourCc(*b"avcC"), &record))
            );
            assert_eq!(
                crate::avc_max_num_ref_frames_with_planning(&record, Some(&budget)),
                Ok(crate::avc_max_num_ref_frames(&record))
            );
            let frame = [0, 0, 1, 0x65, 0xb8];
            assert_eq!(
                crate::access::annex_b_frame_access_with_planning(&frame, true, Some(&budget)),
                Ok(crate::access::annex_b_frame_access(&frame, true))
            );
        });
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn control_block_is_one_fixed_bookkeeping_allocation() {
        let (budget, counters) = measure(|| PlanningBudget::new(0));
        let header = Layout::new::<[AtomicUsize; 2]>();
        let (layout, _) = header
            .extend(Layout::new::<State>())
            .expect("ledger layout");
        assert_eq!(counters.allocations, 1);
        assert_eq!(counters.bytes, layout.pad_to_align().size() as u64);
        let (cloned, counters) = measure(|| budget.clone());
        assert_eq!(counters.allocations, 0);
        assert_eq!(cloned.used(), 0);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn charges_include_the_ceiling_and_failure_is_sticky() {
        let budget = PlanningBudget::new(5);
        assert_eq!(budget.used(), 0);
        assert!(!budget.failed());
        assert_eq!(budget.charge(2), Ok(()));
        assert_eq!(budget.charge(3), Ok(()));
        assert_eq!(budget.used(), 5);
        assert_eq!(budget.charge(1), Err(PlanningLimit));
        assert_eq!(budget.used(), 5);
        assert!(budget.failed());
        assert_eq!(budget.charge(0), Err(PlanningLimit));
    }

    #[test]
    fn overflowing_charge_fails_without_wrapping() {
        let budget = PlanningBudget::new(usize::MAX);
        assert_eq!(budget.charge(usize::MAX), Ok(()));
        assert_eq!(budget.charge(1), Err(PlanningLimit));
        assert_eq!(budget.used(), usize::MAX);
        assert!(budget.failed());
    }

    #[test]
    fn geometric_growth_charges_every_full_backing_store() {
        let budget = PlanningBudget::new(7 * size_of::<u32>());
        let mut vector = Vec::new();
        assert_eq!(budget.reserve(&mut vector, 1), Ok(()));
        vector.push(10u32);
        assert_eq!(vector.capacity(), 1);
        assert_eq!(budget.reserve(&mut vector, 1), Ok(()));
        vector.push(20);
        assert_eq!(vector.capacity(), 2);
        assert_eq!(budget.reserve(&mut vector, 1), Ok(()));
        vector.push(30);
        assert_eq!(vector.capacity(), 4);
        assert_eq!(vector, [10, 20, 30]);
        assert_eq!(budget.used(), (1 + 2 + 4) * size_of::<u32>());
        assert!(!budget.failed());
        drop(vector);
        assert_eq!(budget.used(), 7 * size_of::<u32>());
    }

    #[test]
    fn exact_growth_uses_length_not_old_capacity() {
        let budget = PlanningBudget::new(9 * size_of::<u64>());
        let mut vector = Vec::new();
        assert_eq!(budget.reserve_exact(&mut vector, 4), Ok(()));
        vector.extend_from_slice(&[1u64, 2]);
        assert_eq!(budget.reserve_exact(&mut vector, 3), Ok(()));
        assert_eq!(vector.capacity(), 5);
        assert_eq!(vector, [1, 2]);
        assert_eq!(budget.used(), (4 + 5) * size_of::<u64>());
        assert_eq!(budget.reserve_exact(&mut vector, 3), Ok(()));
        assert_eq!(budget.used(), 9 * size_of::<u64>());
    }

    #[test]
    fn large_additional_request_outgrows_doubling() {
        let budget = PlanningBudget::new(11);
        let mut vector = Vec::new();
        assert_eq!(budget.reserve(&mut vector, 1), Ok(()));
        vector.push(1u8);
        assert_eq!(budget.reserve(&mut vector, 9), Ok(()));
        assert_eq!(vector.capacity(), 10);
        assert_eq!(budget.used(), 11);
    }

    #[test]
    fn refusal_precedes_allocation_and_preserves_the_vector() {
        let budget = PlanningBudget::new(2);
        let mut vector = Vec::new();
        assert_eq!(budget.reserve_exact(&mut vector, 1), Ok(()));
        vector.push(7u8);
        let (result, counters) = measure(|| budget.reserve(&mut vector, 1));
        assert_eq!(result, Err(PlanningLimit));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert_eq!(vector.capacity(), 1);
        assert_eq!(vector, [7]);
        assert_eq!(budget.used(), 1);
        assert!(budget.failed());
        assert_eq!(budget.reserve_exact(&mut vector, 0), Err(PlanningLimit));
    }

    #[test]
    fn exact_refusal_precedes_allocation() {
        let budget = PlanningBudget::new(3);
        let mut vector = Vec::<u32>::new();
        let (result, counters) = measure(|| budget.reserve_exact(&mut vector, 1));
        assert_eq!(result, Err(PlanningLimit));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert_eq!(vector.capacity(), 0);
        assert_eq!(budget.used(), 0);
        assert!(budget.failed());
    }

    #[test]
    fn element_count_overflow_is_sticky() {
        let budget = PlanningBudget::new(usize::MAX);
        let mut vector = Vec::new();
        assert_eq!(budget.reserve_exact(&mut vector, 1), Ok(()));
        vector.push(1u8);
        let (result, counters) = measure(|| budget.reserve_exact(&mut vector, usize::MAX));
        assert_eq!(result, Err(PlanningLimit));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert_eq!(vector, [1]);
        assert_eq!(budget.used(), 1);
        assert!(budget.failed());
    }

    #[test]
    fn layout_byte_overflow_and_isize_limit_precede_allocation() {
        for additional in [usize::MAX / size_of::<u64>() + 1, isize::MAX as usize] {
            let budget = PlanningBudget::new(usize::MAX);
            let mut vector = Vec::<u64>::new();
            let (result, counters) = measure(|| budget.reserve_exact(&mut vector, additional));
            assert_eq!(result, Err(PlanningLimit));
            assert_eq!(counters.allocations, 0);
            assert_eq!(counters.reallocations, 0);
            assert_eq!(vector.capacity(), 0);
            assert_eq!(budget.used(), 0);
            assert!(budget.failed());
        }
    }

    #[test]
    fn zero_sized_elements_need_no_backing_store() {
        let budget = PlanningBudget::new(0);
        let mut vector = Vec::<()>::new();
        let (result, counters) = measure(|| budget.reserve(&mut vector, usize::MAX));
        assert_eq!(result, Ok(()));
        assert_eq!(counters.allocations, 0);
        assert_eq!(counters.reallocations, 0);
        assert_eq!(budget.used(), 0);
        vector.push(());
        assert_eq!(
            budget.reserve_exact(&mut vector, usize::MAX),
            Err(PlanningLimit)
        );
        assert!(budget.failed());
    }
}
