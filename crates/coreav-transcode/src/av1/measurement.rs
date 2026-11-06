use std::{
    io::{self, Write},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

use coreav_backend_svt_av1::{Error as BackendError, protocol::Request, worker};
use coreav_decode::{Error, ErrorKind};
use coreav_frame::Pixels;

use super::{OutputSink, backend_error};
use crate::encoder::{Chunk, QUEUED_FRAMES, Sink};

pub(crate) struct Decoder {
    inner: Box<dyn coreav_decode::Decoder>,
    elapsed: Duration,
    packets: u64,
    frames: u64,
}

impl Decoder {
    pub(crate) fn new(inner: Box<dyn coreav_decode::Decoder>) -> Self {
        Self {
            inner,
            elapsed: Duration::ZERO,
            packets: 0,
            frames: 0,
        }
    }
}

impl coreav_decode::Decoder for Decoder {
    fn send_packet(&mut self, packet: &coreav_decode::Packet<'_>) -> Result<(), Error> {
        let start = Instant::now();
        let result = self.inner.send_packet(packet);
        self.elapsed += start.elapsed();
        self.packets += u64::from(result.is_ok());
        result
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        let start = Instant::now();
        let result = self.inner.send_eof();
        self.elapsed += start.elapsed();
        result
    }

    fn receive_frame(&mut self) -> Result<Option<coreav_decode::Frame>, Error> {
        let start = Instant::now();
        let result = self.inner.receive_frame();
        self.elapsed += start.elapsed();
        self.frames += u64::from(matches!(result, Ok(Some(_))));
        result
    }

    fn info(&self) -> &coreav_decode::DecoderInfo {
        self.inner.info()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = writeln!(
            io::stderr(),
            "coreav_measurement_decode {{\"seconds\":{},\"packets\":{},\"frames\":{}}}",
            self.elapsed.as_secs_f64(),
            self.packets,
            self.frames
        );
    }
}

pub(super) struct Driver {
    input: SyncSender<Pixels>,
    result: Receiver<Result<OutputSink, Error>>,
    deadline: Instant,
}

struct Writer(OutputSink);

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes).map_err(io::Error::other)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Driver {
    pub(super) fn start(
        request: Request,
        sink: OutputSink,
        deadline: Instant,
        keep_alive: Option<Arc<dyn Send + Sync>>,
    ) -> Result<Self, Error> {
        let (input, frames) = mpsc::sync_channel(QUEUED_FRAMES);
        let (done, result) = mpsc::channel();
        std::thread::Builder::new()
            .name("coreav-av1-measurement".into())
            .spawn(move || {
                let _keep_alive = keep_alive;
                let mut writer = Writer(sink);
                let mut buffer = Vec::new();
                let encoded = worker::measurement_encode(request, &mut writer, || {
                    let pixels = match frames
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(pixels) => pixels,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            return Err(BackendError::Io(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "measurement input deadline",
                            )));
                        }
                    };
                    let mut picture = Vec::new();
                    picture
                        .try_reserve_exact(request.settings.raw_bytes())
                        .map_err(|_| {
                            BackendError::Limit("measurement picture allocation failed")
                        })?;
                    Chunk::Frame {
                        head: Vec::new(),
                        pixels,
                    }
                    .write_to(&mut picture, &mut buffer, None)?;
                    Ok(Some(picture))
                })
                .map_err(backend_error)
                .map(|()| writer.0);
                let _ = done.send(encoded);
            })
            .map_err(|_| {
                Error::new(
                    ErrorKind::Backend,
                    "cannot start measurement encoder thread",
                )
            })?;
        Ok(Self {
            input,
            result,
            deadline,
        })
    }

    pub(super) fn send(&self, pixels: Pixels) -> Result<(), Error> {
        if Instant::now() >= self.deadline {
            return Err(Error::new(
                ErrorKind::Timeout,
                "measurement encoder deadline",
            ));
        }
        self.input
            .send(pixels)
            .map_err(|_| Error::new(ErrorKind::Backend, "measurement encoder stopped"))
    }

    pub(super) fn finish(self) -> Result<OutputSink, Error> {
        drop(self.input);
        self.result
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| {
                Error::new(
                    ErrorKind::Timeout,
                    "measurement encoder completion unavailable",
                )
            })?
    }
}
