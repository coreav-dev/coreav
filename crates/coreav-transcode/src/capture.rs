//! Optional capture of successful writes to an encoder's stdin.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// The registered maximum size of an input capture.
pub const MAX_BYTES: u64 = 64 << 20;

/// A new private file in a caller-owned directory. Existing files are refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target(pub PathBuf);

/// The bytes accepted by stdin, after the writer completed normally.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Receipt {
    /// The capture file; the caller owns its lifetime.
    pub path: PathBuf,
    /// Successfully written bytes, not submitted bytes.
    pub bytes: u64,
    /// SHA-256 of those bytes in order.
    pub sha256: String,
}

pub(crate) struct Recording {
    file: File,
    path: PathBuf,
    bytes: u64,
    hash: Sha256,
    completed: Arc<Mutex<Option<Receipt>>>,
}

pub(crate) struct Pending {
    completed: Arc<Mutex<Option<Receipt>>>,
}

impl Target {
    pub(crate) fn create(&self) -> io::Result<(Recording, Pending)> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&self.0)?;
        let completed = Arc::new(Mutex::new(None));
        Ok((
            Recording {
                file,
                path: self.0.clone(),
                bytes: 0,
                hash: Sha256::new(),
                completed: Arc::clone(&completed),
            },
            Pending { completed },
        ))
    }
}

impl Pending {
    pub(crate) fn finish(self, samples: u64, channels: u8) -> io::Result<Receipt> {
        let expected = samples
            .checked_mul(u64::from(channels))
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n > 0 && *n <= MAX_BYTES)
            .ok_or_else(|| io::Error::other("invalid captured sample count"))?;
        let receipt = self
            .completed
            .lock()
            .map_err(|_| io::Error::other("capture state lost"))?
            .take()
            .ok_or_else(|| io::Error::other("encoder input capture did not complete"))?;
        if receipt.bytes != expected {
            return Err(io::Error::other(
                "captured bytes differ from decoded samples",
            ));
        }
        Ok(receipt)
    }
}

pub(crate) struct RecordedInput<W> {
    stdin: W,
    recording: Recording,
    failed: bool,
}

impl Recording {
    pub(crate) fn attach<W: Write>(self, stdin: W) -> RecordedInput<W> {
        RecordedInput {
            stdin,
            recording: self,
            failed: false,
        }
    }
}

impl<W: Write> RecordedInput<W> {
    pub(crate) fn complete(self) -> io::Result<()> {
        let Self {
            stdin,
            mut recording,
            failed,
        } = self;
        drop(stdin);
        if failed {
            return Err(io::Error::other(
                "encoder input capture had a write or flush error",
            ));
        }
        recording.file.flush()?;
        let sha256 = format!("{:x}", recording.hash.finalize());
        let receipt = Receipt {
            path: recording.path,
            bytes: recording.bytes,
            sha256,
        };
        *recording
            .completed
            .lock()
            .map_err(|_| io::Error::other("capture state lost"))? = Some(receipt);
        Ok(())
    }
}

impl<W: Write> RecordedInput<W> {
    fn accepted(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if u64::try_from(bytes.len())
            .ok()
            .and_then(|n| self.recording.bytes.checked_add(n))
            .is_none_or(|n| n > MAX_BYTES)
        {
            return Err(io::Error::other(
                "encoder input capture exceeds its byte limit",
            ));
        }
        let n = self.stdin.write(bytes)?;
        let accepted = bytes
            .get(..n)
            .ok_or_else(|| io::Error::other("invalid encoder stdin write count"))?;
        self.recording.file.write_all(accepted)?;
        self.recording.hash.update(accepted);
        self.recording.bytes +=
            u64::try_from(n).map_err(|_| io::Error::other("encoder input write count overflow"))?;
        Ok(n)
    }
}

impl<W: Write> Write for RecordedInput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.accepted(bytes);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() != io::ErrorKind::Interrupted)
        {
            self.failed = true;
        }
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        let result = self.stdin.flush();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
}

/// Hashes a file without retaining its contents.
pub fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// SHA-256 of the registration embedded when this binary was compiled.
pub fn registration_sha256() -> String {
    format!(
        "{:x}",
        Sha256::digest(include_bytes!(
            "../../../docs/design/composed-opus-aac-v1.json"
        ))
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "coreav-input-capture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }

        fn open(&self) -> (Recording, Pending) {
            Target(self.0.clone()).create().unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    struct ShortWrites(Vec<u8>);

    impl Write for ShortWrites {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let n = bytes.len().min(3);
            self.0.extend_from_slice(bytes.get(..n).unwrap());
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_writes_capture_only_accepted_bytes_in_order() {
        let scratch = Scratch::new();
        let (recording, pending) = scratch.open();
        let mut writer = recording.attach(ShortWrites(Vec::new()));
        writer.write_all(b"abcdefgh").unwrap();
        assert_eq!(writer.stdin.0, b"abcdefgh");
        writer.complete().unwrap();
        let receipt = pending.finish(2, 1).unwrap();
        assert_eq!(receipt.bytes, 8);
        assert_eq!(std::fs::read(&scratch.0).unwrap(), b"abcdefgh");
        assert_eq!(receipt.sha256, file_sha256(&scratch.0).unwrap());
    }

    #[test]
    fn a_short_accepted_write_is_not_the_submitted_buffer() {
        let scratch = Scratch::new();
        let (recording, pending) = scratch.open();
        let mut writer = recording.attach(ShortWrites(Vec::new()));
        assert_eq!(writer.write(b"abcdefgh").unwrap(), 3);
        writer.complete().unwrap();
        assert_eq!(std::fs::read(&scratch.0).unwrap(), b"abc");
        assert!(pending.finish(2, 1).is_err());
    }

    #[test]
    fn missing_writer_completion_cannot_produce_a_receipt() {
        let scratch = Scratch::new();
        let (recording, pending) = scratch.open();
        let mut writer = recording.attach(Vec::new());
        writer.write_all(b"abcdefgh").unwrap();
        drop(writer);
        assert!(pending.finish(2, 1).is_err());
    }

    #[test]
    fn captures_never_overwrite_an_existing_file() {
        let scratch = Scratch::new();
        std::fs::write(&scratch.0, b"keep").unwrap();
        assert!(Target(scratch.0.clone()).create().is_err());
        assert_eq!(std::fs::read(&scratch.0).unwrap(), b"keep");
    }

    #[test]
    fn an_over_limit_write_never_reaches_stdin() {
        let scratch = Scratch::new();
        let (mut recording, pending) = scratch.open();
        recording.bytes = MAX_BYTES;
        let mut writer = recording.attach(Vec::new());
        assert!(writer.write(b"more").is_err());
        assert!(writer.stdin.is_empty());
        drop(writer);
        assert!(pending.finish(1, 1).is_err());
    }

    #[test]
    fn a_capture_file_error_cannot_be_sealed() {
        let scratch = Scratch::new();
        let (mut recording, pending) = scratch.open();
        recording.file = File::open(&scratch.0).unwrap();
        let mut writer = recording.attach(Vec::new());
        assert!(writer.write_all(b"abcdefgh").is_err());
        assert_eq!(writer.stdin, b"abcdefgh");
        assert!(writer.complete().is_err());
        assert!(pending.finish(2, 1).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_real_writer_delivers_the_same_bytes_and_a_failed_child_stays_failed() {
        use crate::encoder::Process;
        use std::ffi::OsString;
        use std::time::{Duration, Instant};

        for status in [0, 1] {
            let scratch = Scratch::new();
            let (recording, pending) = scratch.open();
            let output = Arc::new(Mutex::new(Vec::<u8>::new()));
            let received = Arc::clone(&output);
            let sink = move |bytes: &[u8]| {
                received.lock().unwrap().extend_from_slice(bytes);
                Ok(())
            };
            let args = [
                OsString::from("-c"),
                OsString::from(format!("/bin/cat; exit {status}")),
            ];
            let program = Path::new("/bin/sh");
            let command = coreav_worker::exec::direct_command(program, &args);
            let mut process = Process::spawn_captured(
                "capture stand-in",
                command,
                program,
                Instant::now() + Duration::from_secs(10),
                sink,
                recording,
            )
            .unwrap();
            process.enqueue(b"abcd".to_vec()).unwrap();
            process.enqueue(b"efgh".to_vec()).unwrap();
            assert_eq!(process.finish().is_ok(), status == 0);
            assert_eq!(*output.lock().unwrap(), b"abcdefgh");
            if status == 0 {
                assert_eq!(
                    pending.finish(2, 1).unwrap().sha256,
                    file_sha256(&scratch.0).unwrap()
                );
            }
        }
    }
}
