use std::sync::{Arc, Mutex};

use coreav_decode::{Error, ErrorKind};
use coreav_frame::{Picture, PictureRecycler, PixelFormat};

#[derive(Debug, Default)]
pub(crate) struct Pool(Mutex<State>);

#[derive(Debug, Default)]
struct State {
    shape: Option<(u32, u32)>,
    bytes: u64,
    slots: usize,
    idle_limit: u64,
    live_bytes: u64,
    live_slots: usize,
    idle_bytes: u64,
    idle: Vec<Picture>,
}

impl Pool {
    pub(crate) fn configure(&self, width: u32, height: u32, bytes: u64, slots: usize) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if state.shape != Some((width, height)) {
            state.idle.clear();
            state.idle_bytes = 0;
            state.shape = Some((width, height));
        }
        state.bytes = bytes;
        state.slots = slots;
    }

    pub(crate) fn pending(&self, bytes: u64) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        state.idle_limit = state.bytes.saturating_sub(bytes);
        while state.idle_bytes > state.idle_limit
            || state.live_bytes.saturating_add(state.idle_bytes) > state.bytes
            || state.live_slots.saturating_add(state.idle.len()) > state.slots
        {
            let Some(picture) = state.idle.pop() else {
                break;
            };
            state.idle_bytes = state
                .idle_bytes
                .saturating_sub(picture.capacity_bytes() as u64);
        }
    }

    pub(crate) fn take(self: &Arc<Self>, width: u32, height: u32) -> Result<Picture, Error> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| Error::new(ErrorKind::Backend, "decoded picture pool is unavailable"))?;
        let picture = match state.idle.pop() {
            Some(picture) => {
                state.idle_bytes = state
                    .idle_bytes
                    .saturating_sub(picture.capacity_bytes() as u64);
                picture
            }
            None => Picture::zeroed(width, height, PixelFormat::YUV420P).map_err(|error| {
                Error::new(
                    ErrorKind::Limit,
                    format!("decoded picture allocation: {error}"),
                )
            })?,
        };
        let bytes = picture.capacity_bytes() as u64;
        if state
            .live_bytes
            .saturating_add(state.idle_bytes)
            .saturating_add(bytes)
            > state.bytes
            || state.live_slots.saturating_add(state.idle.len()) >= state.slots
        {
            return Ok(picture);
        }
        state.live_bytes += bytes;
        state.live_slots += 1;
        let owner: Arc<dyn PictureRecycler> = self.clone();
        Ok(picture.with_recycler(Arc::downgrade(&owner)))
    }
}

impl PictureRecycler for Pool {
    fn recycle(&self, picture: Picture) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        let bytes = picture.capacity_bytes() as u64;
        state.live_bytes = state.live_bytes.saturating_sub(bytes);
        state.live_slots = state.live_slots.saturating_sub(1);
        if state.shape != Some((picture.width(), picture.height()))
            || state.idle_bytes.saturating_add(bytes) > state.idle_limit
            || state
                .live_bytes
                .saturating_add(state.idle_bytes)
                .saturating_add(bytes)
                > state.bytes
            || state.live_slots.saturating_add(state.idle.len()) >= state.slots
            || state.idle.try_reserve(1).is_err()
        {
            return;
        }
        state.idle_bytes += bytes;
        state.idle.push(picture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(slots: usize) -> Arc<Pool> {
        let pool = Arc::new(Pool::default());
        pool.configure(32, 32, 1536 * slots as u64, slots);
        pool.pending(0);
        pool
    }

    #[test]
    fn leases_and_independent_clones_keep_their_samples() {
        let pool = pool(1);
        let mut first = pool.take(32, 32).unwrap();
        first.row_mut(0, 0).unwrap().fill(17);
        let mut cloned = first.clone();
        let plane = first.planes()[0].clone();
        cloned.row_mut(0, 0).unwrap().fill(42);
        let mut held = pool.take(32, 32).unwrap();
        held.row_mut(0, 0).unwrap().fill(99);
        assert_eq!(first.row(0, 0).unwrap(), &[17; 32]);
        let address = first.row(0, 0).unwrap().as_ptr();
        drop(first);
        let mut reused = pool.take(32, 32).unwrap();
        assert_eq!(reused.row(0, 0).unwrap().as_ptr(), address);
        reused.row_mut(0, 0).unwrap().fill(88);
        assert_eq!(cloned.row(0, 0).unwrap(), &[42; 32]);
        assert_eq!(&plane.data()[..32], &[17; 32]);
        assert_eq!(held.row(0, 0).unwrap(), &[99; 32]);
        let state = pool.0.lock().unwrap();
        assert_eq!(state.live_bytes, 1536);
        assert_eq!(state.live_slots, 1);
    }

    #[test]
    fn late_returns_geometry_changes_and_pending_bytes_bound_idle_storage() {
        let pool = pool(2);
        let first = pool.take(32, 32).unwrap();
        let second = pool.take(32, 32).unwrap();
        pool.pending(1536);
        drop(first);
        drop(second);
        assert_eq!(pool.0.lock().unwrap().idle_bytes, 1536);
        pool.pending(3072);
        assert_eq!(pool.0.lock().unwrap().idle_bytes, 0);
        let old = pool.take(32, 32).unwrap();
        pool.configure(16, 16, 384, 1);
        pool.pending(0);
        let small = pool.take(16, 16).unwrap();
        drop(old);
        assert_eq!(pool.0.lock().unwrap().idle_bytes, 0);
        drop(small);
        let small = pool.take(16, 16).unwrap();
        assert_eq!(small.packed_len(), 384);
        drop(pool);
        assert_eq!(small.row(0, 0).unwrap(), &[0; 16]);
    }

    #[test]
    fn asynchronous_consumers_return_storage_only_after_completion() {
        let pool = pool(1);
        let picture = pool.take(32, 32).unwrap();
        let (sent, received) = std::sync::mpsc::sync_channel(0);
        let writer = std::thread::spawn(move || {
            received.recv().unwrap();
            drop(picture);
        });
        for _ in 0..8 {
            drop(pool.take(32, 32).unwrap());
            assert_eq!(pool.0.lock().unwrap().idle_bytes, 0);
        }
        sent.send(()).unwrap();
        writer.join().unwrap();
        assert_eq!(pool.0.lock().unwrap().idle_bytes, 1536);
    }
}
