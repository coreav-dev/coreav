#![forbid(unsafe_code)]

use crate::{Error, Result};
use std::sync::{Mutex, OnceLock, mpsc};

pub(crate) fn run<T: Send + Sync>(
    width: usize,
    height: usize,
    threads: usize,
    compute: impl Fn(usize, &[OnceLock<T>]) -> Result<T> + Sync,
) -> Result<Vec<OnceLock<T>>> {
    if width == 0 || height == 0 || threads == 0 {
        return Err(Error::Internal);
    }
    let len = width.checked_mul(height).ok_or(Error::Internal)?;
    let mut table = Vec::new();
    table
        .try_reserve_exact(len)
        .map_err(|_| Error::Allocation)?;
    table.resize_with(len, OnceLock::new);
    let workers = threads.min(height).min(width.div_ceil(2));
    if workers == 1 {
        for address in 0..table.len() {
            let value = compute(address, &table)?;
            table
                .get(address)
                .ok_or(Error::Internal)?
                .set(value)
                .map_err(|_| Error::Internal)?;
        }
        return Ok(table);
    }
    let mut progress = Vec::new();
    progress
        .try_reserve_exact(height)
        .map_err(|_| Error::Allocation)?;
    for _ in 0..height {
        let (sender, receiver) = mpsc::sync_channel::<Result<usize>>(width);
        progress.push((sender, Mutex::new(receiver)));
    }
    let mut outcomes = Vec::new();
    outcomes
        .try_reserve_exact(height)
        .map_err(|_| Error::Allocation)?;
    outcomes.resize_with(height, OnceLock::new);
    let next_row = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(workers)
            .map_err(|_| Error::Allocation)?;
        for _ in 0..workers {
            let table = &table;
            let outcomes = &outcomes;
            let progress = &progress;
            let next_row = &next_row;
            let compute = &compute;
            match std::thread::Builder::new().spawn_scoped(scope, move || {
                loop {
                    let row = next_row.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if row >= height {
                        break;
                    }
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<()> {
                            let top = if row == 0 {
                                None
                            } else {
                                Some(
                                    progress
                                        .get(row - 1)
                                        .ok_or(Error::Internal)?
                                        .1
                                        .lock()
                                        .map_err(|_| Error::Internal)?,
                                )
                            };
                            let mut preceding = 0;
                            for x in 0..width {
                                if let Some(top) = &top {
                                    while preceding < (x + 2).min(width) {
                                        preceding = top.recv().map_err(|_| Error::Internal)??;
                                    }
                                }
                                let address = row * width + x;
                                let value = compute(address, table)?;
                                table
                                    .get(address)
                                    .ok_or(Error::Internal)?
                                    .set(value)
                                    .map_err(|_| Error::Internal)?;
                                if row + 1 < height {
                                    progress
                                        .get(row)
                                        .ok_or(Error::Internal)?
                                        .0
                                        .send(Ok(x + 1))
                                        .map_err(|_| Error::Internal)?;
                                }
                            }
                            Ok(())
                        }))
                        .unwrap_or(Err(Error::Internal));
                    if let Err(error) = result {
                        if let Some((sender, _)) = progress.get(row) {
                            let _ = sender.send(Err(error));
                        }
                    }
                    if let Some(outcome) = outcomes.get(row) {
                        let _ = outcome.set(result);
                    }
                }
            }) {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(Error::Allocation);
                }
            }
        }
        for handle in handles {
            handle.join().map_err(|_| Error::Internal)?;
        }
        for outcome in &outcomes {
            (*outcome.get().ok_or(Error::Internal)?)?;
        }
        Ok(())
    })?;
    Ok(table)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn dependencies_are_published_before_dispatch_under_reordered_completions() {
        for (width, height) in [(1, 1), (1, 8), (2, 7), (3, 8), (12, 9)] {
            for threads in [1, 2, 3, 8] {
                let calls = AtomicUsize::new(0);
                let table = run(width, height, threads, |address, table| {
                    let (x, y) = (address % width, address / width);
                    let read = |at: usize| *table[at].get().unwrap();
                    let mut value = address;
                    if x != 0 {
                        value += read(address - 1);
                    }
                    if y != 0 {
                        value += read(address - width);
                        if x + 1 < width {
                            value += read(address - width + 1);
                        }
                    }
                    if address % 3 == 0 {
                        std::thread::yield_now();
                    }
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok(value % 1000)
                })
                .unwrap();
                assert_eq!(calls.load(Ordering::Relaxed), width * height);
                let mut expected = vec![0; width * height];
                for address in 0..expected.len() {
                    let (x, y) = (address % width, address / width);
                    let mut value = address;
                    if x != 0 {
                        value += expected[address - 1];
                    }
                    if y != 0 {
                        value += expected[address - width];
                        if x + 1 < width {
                            value += expected[address - width + 1];
                        }
                    }
                    expected[address] = value % 1000;
                }
                assert_eq!(
                    table.iter().map(|s| *s.get().unwrap()).collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }

    #[test]
    fn earlier_rows_finish_before_selecting_the_first_raster_failure() {
        for threads in [1, 2, 3, 8] {
            let result = run(12, 8, threads, |address, _| {
                if address == 11 {
                    return Err(Error::Coefficient);
                }
                if address == 12 {
                    return Err(Error::MacroblockLimit);
                }
                if address < 11 {
                    std::thread::yield_now();
                }
                Ok(address)
            });
            assert!(matches!(result, Err(Error::Coefficient)));
        }
    }

    #[test]
    fn worker_unwind_is_reported_without_stranding_other_workers() {
        let result = run(12, 8, 8, |address, _| {
            if address == 15 {
                panic!("injected worker unwind");
            }
            Ok(address)
        });
        assert!(matches!(result, Err(Error::Internal)));
        assert!(run(0, 8, 2, |_, _| Ok(())).is_err());
        assert!(run(8, 0, 2, |_, _| Ok(())).is_err());
        assert!(run(8, 8, 0, |_, _| Ok(())).is_err());
        assert!(run(usize::MAX, 2, 1, |_, _| Ok(())).is_err());
    }
}
