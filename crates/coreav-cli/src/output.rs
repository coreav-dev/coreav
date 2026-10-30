//! What the commands that write a file share: the
//! failure of a write with the exit code it maps to, the checks on the
//! output path, and writing to a temporary file that replaces the output
//! only once complete.

use crate::error::Failure;
use coreav_isobmff_write::ExtractError;
use serde_json::{Value, json};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

/// Why a write failed, with the exit code it maps to.
pub enum WriteFailure {
    /// The source was rejected or cannot be written as asked: exit 1.
    Rejected(Failure),
    /// The input could not be read, the output could not be written, or the
    /// source changed between passes: exit 3.
    Io(Value),
    /// An HLS index is malformed, or is not the source's: exit 1.
    Index(Value),
    /// A failure that carries its own `coreav.error/1` object (`coreav
    /// join`, whose errors name the input): exit 3 when `io`, else 1.
    Object {
        /// The `coreav.error/1` object.
        json: Value,
        /// Exit code 3 rather than 1.
        io: bool,
    },
}

impl WriteFailure {
    /// The `coreav.error/1` object.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Rejected(failure) => failure.to_json(),
            Self::Io(value) | Self::Index(value) | Self::Object { json: value, .. } => {
                value.clone()
            }
        }
    }

    /// Exit code 3 rather than 1.
    pub fn is_io(&self) -> bool {
        match self {
            Self::Rejected(failure) => failure.is_io(),
            Self::Io(_) => true,
            Self::Index(_) => false,
            Self::Object { io, .. } => *io,
        }
    }

    /// Strict-safe mode's refusal: exit status 4.
    pub fn is_strict_safe(&self) -> bool {
        matches!(self, Self::Rejected(failure) if failure.is_strict_safe())
    }

    /// An output that could not be created, written or put in place.
    pub fn output(error: &std::io::Error) -> Self {
        Self::Io(io_object(&format!("write error: {}", error.kind())))
    }
}

impl From<ExtractError> for WriteFailure {
    fn from(error: ExtractError) -> Self {
        match error {
            ExtractError::Source(error) => Self::Rejected(Failure::library(error)),
            ExtractError::Write(kind) => Self::Io(io_object(&format!("write error: {kind}"))),
            ExtractError::SourceChanged => {
                Self::Io(io_object("the source changed while it was copied"))
            }
            ExtractError::InvalidIndex { reason } => Self::Index(json!({
                "schema": "coreav.error/1",
                "kind": "invalid_index",
                "limit": null,
                "offset": null,
                "message": format!("invalid HLS index: {reason}"),
            })),
        }
    }
}

impl From<coreav_isobmff::Error> for WriteFailure {
    fn from(error: coreav_isobmff::Error) -> Self {
        Self::Rejected(Failure::library(error))
    }
}

fn io_object(message: &str) -> Value {
    json!({
        "schema": "coreav.error/1",
        "kind": "io",
        "limit": null,
        "offset": null,
        "message": message,
    })
}

/// Why `output` may not be written: it names the input file, or it exists
/// and `force` is not set. The message is for a usage error (exit 2).
pub fn refuse(input: &Path, output: &Path, force: bool) -> Option<String> {
    // A directory cannot be renamed onto: refused before any work, with
    // or without `--force`.
    // A symbolic link to a directory is not one: `--force` replaces the link.
    if std::fs::symlink_metadata(output).is_ok_and(|m| m.is_dir()) {
        return Some(format!("{} is a directory", output.display()));
    }
    if same_file(input, output) {
        return Some(format!(
            "{} is the input file; write the output to another path",
            output.display()
        ));
    }
    if !force && std::fs::symlink_metadata(output).is_ok() {
        return Some(format!(
            "{} exists; pass --force to overwrite it",
            output.display()
        ));
    }
    None
}

/// Whether two paths name the same existing file (the same path, a link to
/// it, or on Unix another hard link to it).
pub fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        if x == y {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return x.dev() == y.dev() && x.ino() == y.ino();
        }
    }
    false
}

/// The longest file name, in bytes, that common file systems accept (255 on
/// ext4, APFS, XFS and NTFS, where it counts UTF-16 units, which are never
/// more than the UTF-8 bytes).
const MAX_NAME_BYTES: usize = 255;

/// What tells this process's temporary files apart: the process id, or on
/// WASI, where a module has none (`std::process::id` panics there), the
/// clock's nanoseconds when it is first asked. Opening with `create_new`
/// still refuses a name in use.
fn process_tag() -> u32 {
    #[cfg(not(target_os = "wasi"))]
    let tag = std::process::id();
    #[cfg(target_os = "wasi")]
    let tag = {
        static TAG: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        *TAG.get_or_init(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos())
        })
    };
    tag
}

/// A new file name next to `path` for the output while it is written:
/// `.{name}.coreav-{tag}.part` ([`process_tag`]), with `{name}` shortened on a character
/// boundary so that the whole name fits in [`MAX_NAME_BYTES`] even when the
/// output's own name is close to the limit.
pub fn temporary_path(path: &Path) -> PathBuf {
    tagged_temporary_path(path, "")
}

/// [`temporary_path`] with `tag` in the suffix, for a command that writes
/// several outputs at once (`coreav split`), whose shortened names could
/// otherwise collide.
pub fn tagged_temporary_path(path: &Path, tag: &str) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| "output".into(), |n| n.to_string_lossy().into_owned());
    let suffix = format!(".coreav-{}{tag}.part", process_tag());
    let room = MAX_NAME_BYTES
        .saturating_sub(suffix.len())
        .saturating_sub(1);
    let mut end = name.len().min(room);
    while !name.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let kept = name.get(..end).unwrap_or_default();
    path.with_file_name(format!(".{kept}{suffix}"))
}

/// Runs `write` on a new file next to `path` and moves it to `path` once
/// `write` succeeds ([`place`]), so a failure never touches an existing file
/// and leaves no partial one. Check `path` with
/// [`refuse`] first, with the same `force`.
pub fn to_file<T>(
    path: &Path,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<T, WriteFailure>,
) -> Result<T, WriteFailure> {
    to_file_patched(path, force, |out| write(out).map(|value| (value, None)))
}

/// [`to_file`] for an output whose first bytes are known only at its end,
/// such as a WAV header's sizes: `write` returns them with its value, and
/// they are written over the start of the file before it is moved into
/// place. Standard output gets the bytes as first written.
pub fn to_file_patched<T>(
    path: &Path,
    force: bool,
    write: impl FnOnce(&mut dyn Write) -> Result<(T, Option<Vec<u8>>), WriteFailure>,
) -> Result<T, WriteFailure> {
    let temporary = temporary_path(path);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        // `Failure::io` is an I/O failure (exit 3) despite the variant name.
        .map_err(|e| WriteFailure::Rejected(Failure::io(e)))?;
    let mut out = BufWriter::new(file);
    // Flushed and closed before the rename; on failure `out` is dropped
    // with the closure, so the file is closed before it is removed.
    let result = write(&mut out)
        .and_then(|(value, patch)| match out.into_inner() {
            Ok(mut file) => match patch {
                Some(bytes) => {
                    use std::io::{Seek, SeekFrom};
                    file.seek(SeekFrom::Start(0))
                        .and_then(|_| file.write_all(&bytes))
                        .map_err(|e| WriteFailure::output(&e))?;
                    Ok(value)
                }
                None => Ok(value),
            },
            Err(error) => Err(WriteFailure::output(error.error())),
        })
        .and_then(|value| match place(&temporary, path, force) {
            Ok(()) => Ok(value),
            Err(error) => Err(WriteFailure::output(&error)),
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Moves the written `temporary` file to `path`. With `force` it replaces
/// what is there (a symbolic link is replaced, not written through) and
/// keeps the replaced file's permissions. Without it, the move is a hard
/// link, which fails rather than replace a file created at `path` after
/// the command checked it ([`refuse`]); a file system without hard links
/// gets a rename after one more check.
pub fn place(temporary: &Path, path: &Path, force: bool) -> std::io::Result<()> {
    if force {
        if let Ok(old) = std::fs::symlink_metadata(path) {
            if old.is_file() {
                std::fs::set_permissions(temporary, kept_permissions(&old))?;
            }
        }
        return std::fs::rename(temporary, path);
    }
    match std::fs::hard_link(temporary, path) {
        Ok(()) => {
            // The output is in place; a leftover temporary file is not a
            // failure.
            let _ = std::fs::remove_file(temporary);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(error),
        Err(_) if std::fs::symlink_metadata(path).is_err() => std::fs::rename(temporary, path),
        Err(error) => Err(error),
    }
}

/// The replaced file's permissions, without the set-user-ID, set-group-ID
/// and sticky bits on Unix: only who may read, write and run it.
fn kept_permissions(old: &std::fs::Metadata) -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::Permissions::from_mode(old.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        old.permissions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_name(name: &str) -> String {
        let path = temporary_path(&Path::new("dir").join(name));
        assert_eq!(path.parent(), Some(Path::new("dir")));
        path.file_name().unwrap().to_str().unwrap().to_owned()
    }

    #[test]
    fn temporary_names_fit_the_limit() {
        let suffix = format!(".coreav-{}.part", process_tag());
        assert_eq!(temporary_name("clip.mp4"), format!(".clip.mp4{suffix}"));
        for name in [
            "a".repeat(255),
            // Fits alone, too long only with the prefix and suffix.
            "a".repeat(240),
            format!("{}.mp4", "a".repeat(251)),
            "한".repeat(85),
            format!("a{}", "é".repeat(127)),
        ] {
            let temporary = temporary_name(&name);
            assert!(temporary.len() <= MAX_NAME_BYTES, "{}", temporary.len());
            assert!(temporary.starts_with('.') && temporary.ends_with(&suffix));
            let kept = &temporary[1..temporary.len() - suffix.len()];
            assert!(name.starts_with(kept));
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("coreav-output-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Without `force`, a file that appeared at the output after the checks
    /// is not replaced (#64).
    #[test]
    fn a_file_created_meanwhile_is_not_replaced() {
        let dir = scratch("race");
        let path = dir.join("out.mp4");
        let result = to_file(&path, false, |out| {
            std::fs::write(&path, b"theirs").unwrap();
            out.write_all(b"ours").map_err(|e| WriteFailure::output(&e))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"theirs");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "the temporary file is removed"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// With `force`, the replaced file's permissions are kept (#64).
    #[cfg(unix)]
    #[test]
    fn a_forced_overwrite_keeps_the_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("force");
        let path = dir.join("out.mp4");
        std::fs::write(&path, b"old").unwrap();
        // The set-user-ID bit is not carried over.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o4750)).unwrap();
        let written = to_file(&path, true, |out| {
            out.write_all(b"new").map_err(|e| WriteFailure::output(&e))
        });
        assert!(written.is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o750
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A directory is refused before any work, with or without `force` (#64).
    #[test]
    fn a_directory_output_is_refused() {
        let dir = scratch("dir");
        let input = dir.join("in.mp4");
        std::fs::write(&input, b"x").unwrap();
        for force in [false, true] {
            let message = refuse(&input, &dir, force).unwrap();
            assert!(message.contains("is a directory"), "{message}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn names_that_are_not_unicode_fit_too() {
        use std::os::unix::ffi::OsStrExt;
        // Each invalid byte becomes a three-byte U+FFFD in the lossy form.
        let name = std::ffi::OsStr::from_bytes(&[0xFF; 255]);
        let path = temporary_path(&Path::new("dir").join(name));
        assert!(path.file_name().unwrap().len() <= MAX_NAME_BYTES);
    }
}
