use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use coreav_frame::{ColorInfo, Picture, PixelFormat};
use coreav_isobmff::Rational;
use coreav_transcode::capture::file_sha256;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Stop;
use crate::error::Failure;
use crate::output::WriteFailure;
use crate::strict_safe::StrictSafe;

const MAX_BYTES: u64 = 2 << 30;
const MAX_FRAMES: usize = 100_000;
const MAX_JSON: usize = 8 << 20;

fn failed(error: io::Error) -> Stop {
    Stop::Failed(WriteFailure::Rejected(Failure::io(error)))
}

fn invalid(reason: &'static str) -> Stop {
    failed(io::Error::other(reason))
}

fn create(path: &Path) -> Result<File, Stop> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(failed)
}

fn write_json(path: &Path, document: &Value) -> Result<(), Stop> {
    let data = serde_json::to_vec(document).map_err(|_| invalid("image capture JSON encoding"))?;
    if data.len() > MAX_JSON {
        return Err(invalid("image capture JSON limit"));
    }
    let mut file = create(path)?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(failed)
}

pub(super) struct Session {
    directory: PathBuf,
    source_sha256: String,
    sealed: bool,
}

impl Session {
    pub(super) fn begin(directory: &Path, input: &Path) -> Result<Self, Stop> {
        let call = directory
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !directory.is_absolute()
            || call.len() != 32
            || !call
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid(
                "image capture requires an absolute fresh call directory",
            ));
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
        strict_safe: StrictSafe,
    ) -> Result<(), Stop> {
        if file_sha256(input).map_err(failed)? != self.source_sha256 {
            return Err(invalid("image capture source changed"));
        }
        let stream = summary
            .get("image_capture_stream")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("image capture has no selected stream"))?;
        let path = self.directory.join("frames.json");
        if fs::metadata(&path).map_err(failed)?.len() > MAX_JSON as u64 {
            return Err(invalid("image capture JSON limit"));
        }
        let frames: Value = serde_json::from_slice(&fs::read(path).map_err(failed)?)
            .map_err(|_| invalid("image capture frame JSON"))?;
        let entries = frames
            .as_array()
            .filter(|frames| !frames.is_empty())
            .ok_or_else(|| invalid("image capture has no frames"))?;
        if entries.len() > MAX_FRAMES
            || summary.get("frames").and_then(Value::as_u64) != u64::try_from(entries.len()).ok()
        {
            return Err(invalid("image capture frame count"));
        }
        let receipt = json!({
            "schema": "coreav.image-input/1",
            "registration": "png-bmp-yuv-v1",
            "registration_sha256": format!("{:x}", Sha256::digest(include_bytes!("../../../../docs/design/png-bmp-yuv-v1.md"))),
            "call": self.directory.file_name().and_then(|s| s.to_str()),
            "source_sha256": self.source_sha256,
            "input": input,
            "harness_sha256": file_sha256(&std::env::current_exe().map_err(failed)?).map_err(failed)?,
            "command": args,
            "cwd": std::env::current_dir().map_err(failed)?,
            "strict_safe": strict_safe.on(),
            "stream": stream,
            "frames": frames,
            "complete": true,
        });
        let pending = self.directory.join("receipt.pending");
        write_json(&pending, &receipt)?;
        fs::hard_link(&pending, self.directory.join("receipt.json")).map_err(failed)?;
        fs::remove_file(pending).map_err(failed)?;
        File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(failed)?;
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

pub(super) struct Recorder {
    directory: PathBuf,
    frames: Vec<Value>,
    bytes: u64,
}

impl Recorder {
    pub(super) fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            frames: Vec::new(),
            bytes: 0,
        }
    }

    pub(super) fn picture(
        &mut self,
        picture: &Picture,
        pts: i64,
        time: Rational,
        source_format: String,
        color: &ColorInfo,
    ) -> Result<(), Stop> {
        if picture.format() != PixelFormat::Rgb24 || self.frames.len() >= MAX_FRAMES {
            return Err(invalid("image capture requires bounded RGB24 frames"));
        }
        let bytes = u64::from(picture.width())
            .checked_mul(u64::from(picture.height()))
            .and_then(|n| n.checked_mul(3))
            .ok_or_else(|| invalid("image capture byte overflow"))?;
        let total = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(|| invalid("image capture byte limit"))?;
        self.frames
            .try_reserve(1)
            .map_err(|_| invalid("image capture allocation"))?;
        let name = format!("{:06}.rgb", self.frames.len());
        let path = self.directory.join(&name);
        let mut file = create(&path)?;
        for row in picture.rows(0) {
            file.write_all(row).map_err(failed)?;
        }
        file.sync_all().map_err(failed)?;
        if file.metadata().map_err(failed)?.len() != bytes {
            return Err(invalid("image capture packed length"));
        }
        self.frames.push(json!({
            "ordinal": self.frames.len(), "pts": pts,
            "presentation_time": {"num": time.num, "den": time.den},
            "source_format": source_format,
            "color": {"matrix": color.matrix, "full_range": color.full_range,
                      "chroma_location": color.chroma_location.map(|s| format!("{s:?}"))},
            "width": picture.width(), "height": picture.height(), "pix_fmt": "rgb24",
            "rgb_file": name, "rgb_bytes": bytes, "rgb_sha256": file_sha256(&path).map_err(failed)?,
        }));
        self.bytes = total;
        Ok(())
    }

    pub(super) fn output(&mut self, path: &Path) -> Result<(), Stop> {
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| invalid("image capture missing picture"))?;
        let object = frame
            .as_object_mut()
            .ok_or_else(|| invalid("image capture frame object"))?;
        object.insert(
            "output".to_owned(),
            json!(
                path.file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| invalid("image capture output name"))?
            ),
        );
        object.insert(
            "output_sha256".to_owned(),
            json!(file_sha256(path).map_err(failed)?),
        );
        Ok(())
    }

    pub(super) fn finish(self) -> Result<(), Stop> {
        if self
            .frames
            .iter()
            .any(|frame| frame.get("output_sha256").is_none())
        {
            return Err(invalid("image capture incomplete frame"));
        }
        write_json(
            &self.directory.join("frames.json"),
            &Value::Array(self.frames),
        )
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use coreav_frame::Plane;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "coreav-image-capture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join("source"), b"source").unwrap();
            Self(directory)
        }

        fn session(&self) -> Session {
            Session::begin(
                &self.0.join("0123456789abcdef0123456789abcdef"),
                &self.0.join("source"),
            )
            .ok()
            .unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn padded_picture() -> Picture {
        Picture::new(
            2,
            2,
            PixelFormat::Rgb24,
            vec![Plane::new(
                vec![1, 2, 3, 4, 5, 6, 99, 99, 7, 8, 9, 10, 11, 12, 99, 99],
                8,
            )],
        )
        .unwrap()
    }

    fn finished(scratch: &Scratch, session: &Session) {
        let mut recorder = Recorder::new(&session.directory);
        recorder
            .picture(
                &padded_picture(),
                1,
                Rational::new(1, 25).unwrap(),
                "yuv420p".to_owned(),
                &ColorInfo::default(),
            )
            .ok()
            .unwrap();
        let output = scratch.0.join("image.png");
        fs::write(&output, b"encoded image").unwrap();
        recorder.output(&output).ok().unwrap();
        recorder.finish().ok().unwrap();
    }

    #[test]
    fn capture_packs_active_rows_and_seals_only_after_output() {
        let scratch = Scratch::new();
        let session = scratch.session();
        let directory = session.directory.clone();
        finished(&scratch, &session);
        assert!(!directory.join("receipt.json").exists());
        assert_eq!(
            fs::read(directory.join("000000.rgb")).unwrap(),
            (1u8..=12).collect::<Vec<_>>()
        );
        session
            .seal(
                &json!({"frames":1,"image_capture_stream":2}),
                &["-i".into(), "source".into()],
                &scratch.0.join("source"),
                StrictSafe::new(false),
            )
            .ok()
            .unwrap();
        let receipt: Value =
            serde_json::from_slice(&fs::read(directory.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["complete"], true);
        assert_eq!(receipt["stream"], 2);
        assert_eq!(receipt["frames"][0]["rgb_bytes"], 12);
        assert_eq!(
            receipt["frames"][0]["output_sha256"],
            file_sha256(&scratch.0.join("image.png")).unwrap()
        );
        assert_eq!(
            receipt["frames"][0]["rgb_sha256"],
            file_sha256(&directory.join("000000.rgb")).unwrap()
        );
        assert!(!directory.join("receipt.pending").exists());
    }

    #[test]
    fn changed_source_or_incomplete_output_cannot_leave_a_receipt() {
        let scratch = Scratch::new();
        let session = scratch.session();
        let directory = session.directory.clone();
        finished(&scratch, &session);
        fs::write(scratch.0.join("source"), b"changed").unwrap();
        assert!(
            session
                .seal(
                    &json!({"frames":1,"image_capture_stream":0}),
                    &[],
                    &scratch.0.join("source"),
                    StrictSafe::new(false)
                )
                .is_err()
        );
        assert!(!directory.exists());
        let session = scratch.session();
        let directory = session.directory.clone();
        let mut recorder = Recorder::new(&directory);
        recorder
            .picture(
                &padded_picture(),
                0,
                Rational::from_int(0),
                "yuv420p".into(),
                &ColorInfo::default(),
            )
            .ok()
            .unwrap();
        assert!(recorder.finish().is_err());
        drop(session);
        assert!(!directory.exists());
    }

    #[test]
    fn existing_calls_and_capture_limits_are_not_overwritten() {
        let scratch = Scratch::new();
        let session = scratch.session();
        assert!(Session::begin(&session.directory, &scratch.0.join("source")).is_err());
        let mut recorder = Recorder::new(&session.directory);
        recorder.bytes = MAX_BYTES;
        assert!(
            recorder
                .picture(
                    &padded_picture(),
                    0,
                    Rational::from_int(0),
                    "yuv420p".into(),
                    &ColorInfo::default()
                )
                .is_err()
        );
        assert!(!session.directory.join("000000.rgb").exists());
        assert!(
            write_json(
                &session.directory.join("oversized.json"),
                &json!("x".repeat(MAX_JSON))
            )
            .is_err()
        );
        assert!(!session.directory.join("oversized.json").exists());
    }
}
