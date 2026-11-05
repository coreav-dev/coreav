use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::time::Instant;

use rustix::fs::{MemfdFlags, Mode, OFlags, SealFlags, fcntl_add_seals, memfd_create, open};

use super::{Chunk, EncoderSettings, Error, ErrorKind, Shared, Timing, find_x264, x264_args};

const INPUT_MAGIC: &[u8; 8] = b"CAVSHM01";
const OUTPUT_MAGIC: &[u8; 8] = b"CAVOUT01";
const MAX_DATA: usize = 1 << 20;

pub(super) fn args(settings: &EncoderSettings) -> Result<Vec<String>, Error> {
    let (num, den, variable) = match settings.timing {
        Timing::Constant => (
            u64::from(settings.frame_rate.den),
            u64::from(settings.frame_rate.num),
            false,
        ),
        Timing::Timestamps { .. } => (1, 1_000_000_000, true),
    };
    let mut args = vec![
        "--coreav-shm".into(),
        "--coreav-timebase".into(),
        format!("{num}/{den}"),
        "--coreav-vfr".into(),
        u8::from(variable).to_string(),
    ];
    if variable {
        let clock_program = find_x264().ok_or_else(|| {
            Error::new(
                ErrorKind::Unsupported,
                "the shared-memory helper needs the x264 program for its timecode clock",
            )
        })?;
        let clock_program = std::fs::canonicalize(clock_program).map_err(|e| {
            Error::new(
                ErrorKind::Io,
                format!("resolving the helper's clock program: {e}"),
            )
        })?;
        args.extend([
            "--coreav-clock-program".into(),
            clock_program.to_string_lossy().into_owned(),
        ]);
    }
    args.extend(x264_args(settings)?);
    Ok(args)
}

fn snapshot(frame: &Chunk, expected: usize, buffer: &mut Vec<u8>) -> io::Result<File> {
    if !matches!(frame, Chunk::Frame { head, .. } if head.is_empty()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a helper frame with stream headers",
        ));
    }
    let fd = memfd_create(
        c"coreav-encoder-frame",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let direct = fd.as_fd().try_clone_to_owned()?;
    let mut file = File::from(fd);
    frame.write_to(&mut file, buffer, Some(&direct))?;
    drop(direct);
    if file.metadata()?.len() != expected as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "wrong helper snapshot size",
        ));
    }
    fcntl_add_seals(
        &file,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )?;
    Ok(file)
}

fn line(reader: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    reader.take(129).read_until(b'\n', &mut line)?;
    if line.is_empty() || line.len() > 128 || line.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid helper timecode line",
        ));
    }
    Ok(line)
}

fn timecodes(settings: &EncoderSettings) -> io::Result<Option<BufReader<File>>> {
    let Timing::Timestamps { timecodes, .. } = &settings.timing else {
        return Ok(None);
    };
    let path = timecodes
        .as_ref()
        .ok_or_else(|| io::Error::other("helper timecodes are missing"))?;
    let file = File::from(open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "helper timecodes must be a regular file",
        ));
    }
    let mut reader = BufReader::new(file);
    if line(&mut reader)?.trim_ascii() != b"# timecode format v2" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "helper requires v2 timecodes",
        ));
    }
    Ok(Some(reader))
}

fn timestamp(sequence: u64, reader: &mut Option<impl BufRead>) -> io::Result<i64> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid helper frame time");
    let Some(reader) = reader else {
        return i64::try_from(sequence - 1).map_err(|_| invalid());
    };
    let bytes = line(reader)?;
    let text = std::str::from_utf8(bytes.trim_ascii()).map_err(|_| invalid())?;
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    let milliseconds = whole.parse::<u128>().map_err(|_| invalid())?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().map_err(|_| invalid())? * 10u128.pow((6 - fraction.len()) as u32)
    };
    milliseconds
        .checked_mul(1_000_000)
        .and_then(|n| n.checked_add(fraction))
        .and_then(|n| i64::try_from(n).ok())
        .ok_or_else(invalid)
}

pub(super) fn write_frames(
    mut stdin: impl Write,
    shared: &Shared,
    settings: &EncoderSettings,
    deadline: Instant,
) {
    let result = (|| -> io::Result<()> {
        stdin.write_all(INPUT_MAGIC)?;
        let expected = settings
            .frame_bytes()
            .map_err(|e| io::Error::other(e.message))?;
        let mut buffer = Vec::new();
        let mut times = timecodes(settings)?;
        let mut previous = None;
        loop {
            let (frame, sequence) = {
                let mut queue = shared
                    .queue
                    .lock()
                    .map_err(|_| io::Error::other("lost helper queue"))?;
                loop {
                    if queue.stopped {
                        return Err(io::ErrorKind::BrokenPipe.into());
                    }
                    if let Some(frame) = queue.frames.pop_front() {
                        let sequence = queue
                            .written
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("helper sequence overflow"))?;
                        break (frame, sequence);
                    }
                    if queue.input_done {
                        return Ok(());
                    }
                    queue = shared
                        .changed
                        .wait(queue)
                        .map_err(|_| io::Error::other("lost helper queue"))?;
                }
            };
            shared.changed.notify_all();
            let time = match &frame {
                Chunk::Frame { .. } => timestamp(sequence, &mut times)?,
                Chunk::Bytes(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "helper expected a frame",
                    ));
                }
            };
            if previous.is_some_and(|last| time <= last) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "helper frame times must increase",
                ));
            }
            previous = Some(time);
            let file = snapshot(&frame, expected, &mut buffer)?;
            drop(frame);
            let path = format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd());
            let length = u16::try_from(path.len())
                .ok()
                .filter(|n| *n <= 128)
                .ok_or_else(|| io::Error::other("helper snapshot path too long"))?;
            {
                let mut queue = shared
                    .queue
                    .lock()
                    .map_err(|_| io::Error::other("lost helper queue"))?;
                if queue.stopped {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                queue.pending_snapshot = Some(sequence);
            }
            let mut message = Vec::with_capacity(27 + path.len());
            message.push(1);
            message.extend_from_slice(&sequence.to_le_bytes());
            message.extend_from_slice(&time.to_le_bytes());
            message.extend_from_slice(&(expected as u64).to_le_bytes());
            message.extend_from_slice(&length.to_le_bytes());
            message.extend_from_slice(path.as_bytes());
            stdin.write_all(&message)?;
            stdin.flush()?;
            let mut queue = shared
                .queue
                .lock()
                .map_err(|_| io::Error::other("lost helper queue"))?;
            while !queue.stopped && queue.acknowledged_snapshot != sequence {
                queue = shared
                    .changed
                    .wait(queue)
                    .map_err(|_| io::Error::other("lost helper queue"))?;
            }
            if queue.stopped {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            queue.written = sequence;
            drop(queue);
            drop(file);
        }
    })();
    if let Err(error) = result {
        if Instant::now() >= deadline {
            shared.record(super::timeout());
        } else if error.kind() != io::ErrorKind::BrokenPipe {
            shared.record(Error::new(
                ErrorKind::Io,
                format!("writing shared encoder input: {error}"),
            ));
        }
        shared.stop();
    }
}

pub(super) struct Output<'a, R> {
    input: R,
    shared: &'a Shared,
    started: bool,
    remaining: usize,
    deadline: Instant,
}

impl<'a, R: Read> Output<'a, R> {
    pub(super) fn new(input: R, shared: &'a Shared, deadline: Instant) -> Self {
        Self {
            input,
            shared,
            started: false,
            remaining: 0,
            deadline,
        }
    }

    fn read_protocol(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let invalid =
            || io::Error::new(io::ErrorKind::InvalidData, "invalid shared encoder output");
        if !self.started {
            let mut magic = [0; 8];
            self.input.read_exact(&mut magic)?;
            if &magic != OUTPUT_MAGIC {
                return Err(invalid());
            }
            self.started = true;
        }
        loop {
            if self.remaining != 0 {
                let length = output.len().min(self.remaining);
                let n = self
                    .input
                    .read(output.get_mut(..length).ok_or_else(invalid)?)?;
                if n == 0 {
                    return Err(io::ErrorKind::UnexpectedEof.into());
                }
                self.remaining -= n;
                return Ok(n);
            }
            let mut tag = [0];
            if self.input.read(&mut tag)? == 0 {
                let queue = self.shared.queue.lock().map_err(|_| invalid())?;
                return if queue.input_done
                    && queue.frames.is_empty()
                    && queue.pending_snapshot.is_none()
                {
                    Ok(0)
                } else {
                    Err(invalid())
                };
            }
            match tag {
                [1] => {
                    let mut bytes = [0; 8];
                    self.input.read_exact(&mut bytes)?;
                    let sequence = u64::from_le_bytes(bytes);
                    let mut queue = self.shared.queue.lock().map_err(|_| invalid())?;
                    if queue.pending_snapshot != Some(sequence) {
                        return Err(invalid());
                    }
                    queue.pending_snapshot = None;
                    queue.acknowledged_snapshot = sequence;
                    self.shared.changed.notify_all();
                }
                [2] => {
                    let mut bytes = [0; 4];
                    self.input.read_exact(&mut bytes)?;
                    let length = u32::from_le_bytes(bytes) as usize;
                    if length == 0 || length > MAX_DATA {
                        return Err(invalid());
                    }
                    self.remaining = length;
                }
                _ => return Err(invalid()),
            }
        }
    }
}

impl<R: Read> Read for Output<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let result = self.read_protocol(output);
        if result.is_err() && Instant::now() >= self.deadline {
            self.shared.record(super::timeout());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_frame::{PixelFormat, Pixels, SharedMemory, SharedPicture};
    use std::os::unix::fs::FileExt;
    use std::sync::{Arc, Mutex};

    struct MutablePixels(Mutex<Vec<u8>>);

    #[test]
    fn timecode_values_preserve_exact_generated_nanoseconds_and_reject_bad_lines() {
        for (text, expected) in [
            ("0.000000\n", 0),
            ("33.366667\n", 33_366_667),
            ("1000.123456\n", 1_000_123_456),
        ] {
            let mut reader = Some(std::io::Cursor::new(text.as_bytes()));
            assert_eq!(timestamp(1, &mut reader).unwrap(), expected);
            assert!(timestamp(2, &mut reader).is_err());
        }
        for text in [
            "-1.0\n",
            "1.0000001\n",
            "nan\n",
            "184467440737095516160000.0\n",
            "2.0",
            "\n",
        ] {
            let mut reader = Some(std::io::Cursor::new(text.as_bytes()));
            assert!(timestamp(1, &mut reader).is_err(), "{text}");
        }
        assert!(line(&mut std::io::Cursor::new(vec![b'0'; 129])).is_err());
        assert_eq!(timestamp(3, &mut None::<std::io::Empty>).unwrap(), 2);
    }

    impl SharedMemory for MutablePixels {
        fn read_packed(&self, at: usize, output: &mut [u8]) -> Option<usize> {
            let bytes = self.0.lock().ok()?;
            let rest = bytes.get(at..)?;
            let length = rest.len().min(output.len());
            output
                .get_mut(..length)?
                .copy_from_slice(rest.get(..length)?);
            Some(length)
        }
    }

    #[test]
    fn acknowledged_snapshots_and_forwarded_pages_survive_decoder_reuse() {
        let size = 16 * 16 * 3 / 2;
        let memory = Arc::new(MutablePixels(Mutex::new(vec![37; size])));
        let frame = Chunk::Frame {
            head: Vec::new(),
            pixels: Pixels::Shared(SharedPicture {
                width: 16,
                height: 16,
                format: PixelFormat::YUV420P,
                memory: memory.clone(),
            }),
        };
        let file = snapshot(&frame, size, &mut Vec::new()).unwrap();
        drop(frame);
        let (input, output) = rustix::pipe::pipe().unwrap();
        let (forwarded_input, forwarded_output) = rustix::pipe::pipe().unwrap();
        let mut offset = 0;
        assert_eq!(
            rustix::pipe::splice(
                &file,
                Some(&mut offset),
                &output,
                None,
                size,
                rustix::pipe::SpliceFlags::empty()
            )
            .unwrap(),
            size
        );
        assert_eq!(
            rustix::pipe::tee(
                &input,
                &forwarded_output,
                size,
                rustix::pipe::SpliceFlags::empty()
            )
            .unwrap(),
            size
        );
        let mut consumed = vec![0; size];
        File::from(input).read_exact(&mut consumed).unwrap();
        assert_eq!(rustix::io::ioctl_fionread(&output).unwrap(), 0);
        let mut retained = file.try_clone().unwrap();
        drop(file);
        memory.0.lock().unwrap().fill(99);
        assert!(retained.write_all(&[99]).is_err());
        assert!(retained.set_len(0).is_err());
        assert!(retained.set_len((size + 1) as u64).is_err());
        let mut forwarded = vec![0; size];
        File::from(forwarded_input)
            .read_exact(&mut forwarded)
            .unwrap();
        assert_eq!(consumed, vec![37; size]);
        assert_eq!(forwarded, consumed);
        let mut direct = vec![0; size];
        retained.read_exact_at(&mut direct, 0).unwrap();
        assert_eq!(direct, consumed);
    }

    #[test]
    fn output_rejects_wrong_acknowledgements_and_unbounded_or_short_messages() {
        for tail in [
            [vec![1], 2u64.to_le_bytes().to_vec()].concat(),
            [vec![2], 0u32.to_le_bytes().to_vec()].concat(),
            [vec![2], ((MAX_DATA + 1) as u32).to_le_bytes().to_vec()].concat(),
            vec![3],
            vec![1, 1],
            [vec![2], 2u32.to_le_bytes().to_vec(), vec![9]].concat(),
        ] {
            let shared = Shared::default();
            shared.queue.lock().unwrap().pending_snapshot = Some(1);
            let bytes = [OUTPUT_MAGIC.to_vec(), tail].concat();
            assert!(
                Output::new(
                    bytes.as_slice(),
                    &shared,
                    Instant::now() + std::time::Duration::from_secs(60)
                )
                .read_to_end(&mut Vec::new())
                .is_err()
            );
        }
    }

    #[test]
    fn encoded_bytes_remain_ordered_around_acknowledgements() {
        let shared = Shared::default();
        {
            let mut queue = shared.queue.lock().unwrap();
            queue.pending_snapshot = Some(1);
            queue.input_done = true;
        }
        let wire = [
            OUTPUT_MAGIC.to_vec(),
            vec![2],
            3u32.to_le_bytes().to_vec(),
            b"one".to_vec(),
            vec![1],
            1u64.to_le_bytes().to_vec(),
            vec![2],
            3u32.to_le_bytes().to_vec(),
            b"two".to_vec(),
        ]
        .concat();
        let mut encoded = Vec::new();
        Output::new(
            wire.as_slice(),
            &shared,
            Instant::now() + std::time::Duration::from_secs(60),
        )
        .read_to_end(&mut encoded)
        .unwrap();
        assert_eq!(encoded, b"onetwo");
        let queue = shared.queue.lock().unwrap();
        assert_eq!(queue.pending_snapshot, None);
        assert_eq!(queue.acknowledged_snapshot, 1);
        assert!(
            Output::new(
                b"badmagic".as_slice(),
                &shared,
                Instant::now() + std::time::Duration::from_secs(60)
            )
            .read_to_end(&mut Vec::new())
            .is_err()
        );
    }
}
