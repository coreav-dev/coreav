use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use coreav_transcode::capture::file_sha256;
use serde_json::{Value, json};

use super::Stop;
use crate::error::Failure;
use crate::output::WriteFailure;

pub(super) struct Session {
    directory: PathBuf,
    source_sha256: String,
    sealed: bool,
}

fn failed(error: io::Error) -> Stop {
    Stop::Failed(WriteFailure::Rejected(Failure::io(error)))
}

impl Session {
    pub(super) fn begin(directory: &Path, input: &Path) -> Result<Self, Stop> {
        if directory.to_str().is_none() {
            return Err(failed(io::Error::other(
                "capture directory must be valid UTF-8",
            )));
        }
        let call = directory
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if call.len() != 32
            || !call
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(failed(io::Error::other(
                "capture directory requires a 32-character lowercase hex call identity",
            )));
        }
        let source_sha256 = file_sha256(input).map_err(failed)?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(directory).map_err(failed)?;
        Ok(Self {
            directory: directory.to_owned(),
            source_sha256,
            sealed: false,
        })
    }

    pub(super) fn seal(
        mut self,
        summary: &Value,
        args: &[String],
        input: &Path,
        output: &Path,
    ) -> Result<(), Stop> {
        let capture = summary
            .get("input_capture")
            .filter(|value| value.is_object())
            .ok_or_else(|| {
                failed(io::Error::other(
                    "native transcode produced no input capture",
                ))
            })?;
        let captured_sha256 = capture
            .get("pcm")
            .and_then(|pcm| pcm.get("sha256"))
            .and_then(Value::as_str);
        let pcm = self.directory.join("stdin.f32");
        let actual_pcm = file_sha256(&pcm).map_err(failed)?;
        if captured_sha256 != Some(actual_pcm.as_str())
            || file_sha256(input).map_err(failed)? != self.source_sha256
        {
            return Err(failed(io::Error::other("capture or source bytes changed")));
        }
        let registration_text = include_str!("../../../../docs/design/composed-opus-aac-v1.json");
        let registered: Value = serde_json::from_str(registration_text)
            .map_err(|_| failed(io::Error::other("invalid embedded capture registration")))?;
        let receipt = json!({
            "schema": "coreav.encoder-input/1",
            "version": registered.get("version"),
            "registration_sha256": coreav_transcode::capture::registration_sha256(),
            "call": self.directory.file_name().and_then(|s| s.to_str()),
            "source_sha256": self.source_sha256,
            "output_sha256": file_sha256(output).map_err(failed)?,
            "candidate_sha256": file_sha256(&std::env::current_exe().map_err(failed)?).map_err(failed)?,
            "command": args,
            "audio": summary.get("encoded_audio"),
            "input": capture,
            "complete": true,
        });
        let data = serde_json::to_vec(&receipt)
            .map_err(|_| failed(io::Error::other("cannot serialize input capture")))?;
        let max = registered
            .get("capture")
            .and_then(|c| c.get("max_receipt_bytes"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if u64::try_from(data.len()).unwrap_or(u64::MAX) > max {
            return Err(failed(io::Error::other(
                "input capture receipt exceeds its limit",
            )));
        }
        let pending = self.directory.join("receipt.pending");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file: File = options.open(&pending).map_err(failed)?;
        file.write_all(&data)
            .and_then(|()| file.flush())
            .map_err(failed)?;
        drop(file);
        fs::hard_link(&pending, self.directory.join("receipt.json")).map_err(failed)?;
        fs::remove_file(pending).map_err(failed)?;
        self.sealed = true;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.sealed {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
