use crate::{DecodedPicture, OutputMemory, PictureInfo, Result};

#[cfg(feature = "test-support")]
thread_local! {
    static ROW_OVERLAP: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

#[cfg(feature = "test-support")]
pub fn begin_evidence() {
    ROW_OVERLAP.set(Some(0));
}

#[cfg(feature = "test-support")]
pub fn take_row_overlap() -> Option<u64> {
    ROW_OVERLAP.replace(None)
}

#[cfg(feature = "test-support")]
pub(crate) fn record_row_overlap() {
    if let Some(count) = ROW_OVERLAP.get() {
        ROW_OVERLAP.set(count.checked_add(1));
    }
}

#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admission {
    pub sps_id: u32,
    pub sps_hash: [u8; 32],
    pub ceiling: usize,
    pub workers: usize,
    pub lag: usize,
    pub frame_bytes: usize,
    pub reserved_bytes: usize,
    pub limit_bytes: usize,
}

#[derive(Clone, Copy)]
pub struct OutputHeader {
    pub index: u64,
    pub poc: i32,
    pub timestamp: Option<i64>,
    pub origin: Option<u64>,
    pub info: PictureInfo,
}

#[derive(Clone, Copy)]
pub struct SinkCost {
    pub max_packet_bytes: usize,
    pub max_nals: usize,
    pub max_pending_bytes: u64,
    pub queue_capacity: usize,
    pub queued_frames: usize,
    pub record_bytes: usize,
    pub plane_records_bytes: usize,
    pub owner_bytes: usize,
    pub debt_peak_bytes: usize,
}

pub struct RowReceive {
    pub consume: Option<usize>,
    pub pop: bool,
}

pub trait RowSink {
    fn prepare(
        &mut self,
        header: OutputHeader,
        memory: &mut OutputMemory<'_>,
        bounded: bool,
    ) -> Result<bool>;
    fn reserve_queue(
        &mut self,
        records: usize,
        memory: &mut OutputMemory<'_>,
        admit: &mut dyn FnMut(usize) -> Result<bool>,
    ) -> Result<bool>;
    fn qualify(&self, info: PictureInfo, probe: &mut CapacityProbe) -> bool;
    fn fill(&mut self, front_offset: usize, picture: DecodedPicture<'_>) -> Result<()>;
    fn truncate(&mut self, keep: usize);
    fn cost(&self, debt: usize, frame_bytes: usize) -> Result<SinkCost>;
}

pub struct CapacityProbe {
    limit: usize,
    pub peak_bytes: usize,
}

impl CapacityProbe {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            peak_bytes: 0,
        }
    }

    pub fn check<T>(&mut self, count: usize) -> bool {
        #[cfg(test)]
        if FORCE_CAPACITY_MISMATCH.get() {
            return false;
        }
        if count
            .checked_mul(core::mem::size_of::<T>())
            .is_none_or(|bytes| bytes > self.limit)
        {
            return false;
        }
        let mut buffer = Vec::<T>::new();
        if buffer.try_reserve_exact(count).is_err() {
            return false;
        }
        let Some(bytes) = buffer.capacity().checked_mul(core::mem::size_of::<T>()) else {
            return false;
        };
        self.peak_bytes = self.peak_bytes.max(bytes);
        buffer.capacity() == count && bytes <= self.limit
    }
}

#[cfg(test)]
std::thread_local! {
    pub(crate) static FORCE_CAPACITY_MISMATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
