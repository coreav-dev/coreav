use crate::{DecodedPicture, OutputMemory, PictureInfo, Result};

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
    fn prepare(&mut self, header: OutputHeader, memory: &mut OutputMemory<'_>) -> Result<bool>;
    fn fill(&mut self, front_offset: usize, picture: DecodedPicture<'_>) -> Result<()>;
    fn truncate(&mut self, keep: usize);
    fn cost(&self, debt: usize, frame_bytes: usize) -> Result<SinkCost>;
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_row_lag_has_a_reviewed_standard_allocation_implementation() {
        #[cfg(not(hevc_exact_capacities))]
        panic!(
            "Re-audit RawVec exact growth and Global requested allocation layouts for this rustc before adding its commit hash to coreav-hevc/build.rs; unknown builds retain legacy zero-lag decoding"
        );
        for count in [0, 1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 127, 128, 255, 4096] {
            let mut bytes = Vec::<u8>::new();
            bytes.try_reserve_exact(count).unwrap();
            assert_eq!(bytes.capacity(), count);
            let mut samples = Vec::<u16>::new();
            samples.try_reserve_exact(count).unwrap();
            assert_eq!(samples.capacity(), count);
            let mut records = std::collections::VecDeque::<[usize; 16]>::new();
            records.try_reserve_exact(count).unwrap();
            assert_eq!(records.capacity(), count);
            records.resize(count, [0; 16]);
            records.try_reserve_exact(1).unwrap();
            assert_eq!(records.capacity(), count + 1);
        }
    }
}
