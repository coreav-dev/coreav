#![forbid(unsafe_code)]

use crate::{Error, Result, api::zeros};
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
    let (tasks, receiver) = mpsc::sync_channel::<usize>(workers);
    let receiver = Mutex::new(receiver);
    let (results, completed) = mpsc::sync_channel(workers);
    std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        handles
            .try_reserve_exact(workers)
            .map_err(|_| Error::Allocation)?;
        for _ in 0..workers {
            let receiver = &receiver;
            let results = results.clone();
            let table = &table;
            let compute = &compute;
            match std::thread::Builder::new().spawn_scoped(scope, move || {
                loop {
                    let task = match receiver.lock() {
                        Ok(guard) => guard.recv(),
                        Err(poisoned) => poisoned.into_inner().recv(),
                    };
                    let Ok(address) = task else {
                        break;
                    };
                    let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        compute(address, table)
                    }))
                    .unwrap_or(Err(Error::Internal));
                    if results.send((address, value)).is_err() {
                        break;
                    }
                }
            }) {
                Ok(handle) => handles.push(handle),
                Err(_) => {
                    drop(tasks);
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(Error::Allocation);
                }
            }
        }
        drop(results);
        let result = (|| {
            let mut progress: Vec<usize> = zeros(height)?;
            let mut busy: Vec<bool> = zeros(height)?;
            let mut running = 0;
            let mut failure: Option<(usize, Error)> = None;
            loop {
                for row in 0..height {
                    let x = *progress.get(row).ok_or(Error::Internal)?;
                    if running == workers
                        || x == width
                        || *busy.get(row).ok_or(Error::Internal)?
                        || failure.is_some_and(|(address, _)| row >= address / width)
                        || (row != 0
                            && *progress.get(row - 1).ok_or(Error::Internal)? < (x + 2).min(width))
                    {
                        continue;
                    }
                    tasks.send(row * width + x).map_err(|_| Error::Internal)?;
                    *busy.get_mut(row).ok_or(Error::Internal)? = true;
                    running += 1;
                }
                if running == 0 {
                    break;
                }
                let (address, value) = completed.recv().map_err(|_| Error::Internal)?;
                let row = address / width;
                *busy.get_mut(row).ok_or(Error::Internal)? = false;
                running -= 1;
                match value {
                    Ok(value) => {
                        table
                            .get(address)
                            .ok_or(Error::Internal)?
                            .set(value)
                            .map_err(|_| Error::Internal)?;
                        *progress.get_mut(row).ok_or(Error::Internal)? += 1;
                    }
                    Err(error) => {
                        if failure.is_none_or(|old| address < old.0) {
                            failure = Some((address, error));
                        }
                    }
                }
            }
            if let Some((_, error)) = failure {
                return Err(error);
            }
            if progress.iter().any(|&x| x != width) {
                return Err(Error::Internal);
            }
            Ok(())
        })();
        drop(tasks);
        let mut joined = Ok(());
        for handle in handles {
            if handle.join().is_err() {
                joined = Err(Error::Internal);
            }
        }
        result.and(joined)
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
