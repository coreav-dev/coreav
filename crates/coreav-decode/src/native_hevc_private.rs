use std::collections::VecDeque;

use crate::{Error, Frame};

pub struct OutputFrame {
    pub frame: Frame,
    pub bytes: u64,
    pub memory: usize,
}

pub struct TerminalOutput {
    pub frames: VecDeque<OutputFrame>,
    pub error: Error,
}

impl TerminalOutput {
    pub fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        match self.frames.pop_front() {
            Some(entry) => Ok(Some(entry.frame)),
            None => Err(self.error.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendKind, Checked, Decoder, DecoderInfo, ErrorKind, Isolation, Limits, Packet};
    use coreav_frame::{Chroma, Picture, PixelFormat, Pixels, Plane};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct Live {
        info: DecoderInfo,
        dropped: Arc<AtomicBool>,
        terminal: Option<TerminalOutput>,
        frame: Option<Frame>,
    }

    impl Drop for Live {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl Decoder for Live {
        fn send_packet(&mut self, _: &Packet<'_>) -> Result<(), Error> {
            Err(self.terminal.as_ref().unwrap().error.clone())
        }

        fn send_eof(&mut self) -> Result<(), Error> {
            panic!("live decoder used after handoff")
        }

        fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
            Ok(Some(self.frame.take().expect("unexpected live receive")))
        }

        fn info(&self) -> &DecoderInfo {
            &self.info
        }

        fn take_native_hevc_terminal_output(&mut self) -> Option<TerminalOutput> {
            self.terminal.take()
        }
    }

    fn packed_frame(pts: i64) -> OutputFrame {
        let picture = Picture::new(
            2,
            2,
            PixelFormat::Yuv {
                chroma: Chroma::Cs420,
                depth: 8,
            },
            vec![
                Plane::new(vec![1; 4], 2),
                Plane::new(vec![2], 1),
                Plane::new(vec![3], 1),
            ],
        )
        .unwrap();
        OutputFrame {
            frame: Frame {
                pts: Some(pts),
                color: Default::default(),
                pixels: Pixels::Cpu(picture),
            },
            bytes: 6,
            memory: 6 + 3 * core::mem::size_of::<Plane>(),
        }
    }

    #[test]
    fn checked_terminal_handoff_drops_backend_before_returning_failure() {
        for kind in [ErrorKind::Limit, ErrorKind::InvalidData, ErrorKind::Timeout] {
            let info = DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: "terminal fixture".into(),
            };
            let dropped = Arc::new(AtomicBool::new(false));
            let error = Error::new(kind, "latched failure");
            let output = TerminalOutput {
                frames: [packed_frame(9)].into(),
                error: error.clone(),
            };
            let mut checked = Checked {
                decoder: Some(Box::new(Live {
                    info: info.clone(),
                    dropped: Arc::clone(&dropped),
                    terminal: Some(output),
                    frame: None,
                })),
                info,
                limits: Limits::default(),
                terminal: None,
            };
            let packet = Packet {
                data: &[],
                pts: None,
                dts: None,
                keyframe: false,
                discard: false,
            };
            assert_eq!(checked.send_packet(&packet), Err(error.clone()));
            assert!(dropped.load(Ordering::SeqCst));
            assert!(checked.decoder.is_none());
            assert_eq!(checked.send_eof(), Err(error.clone()));
            assert_eq!(checked.receive_frame().unwrap().unwrap().pts, Some(9));
            assert_eq!(checked.receive_frame().unwrap_err(), error);
            assert_eq!(checked.info().decoder, "terminal fixture");
        }
    }

    #[test]
    fn wrapper_frame_refusal_discards_later_terminal_output() {
        let limits = Limits {
            max_width: 1,
            ..Limits::default()
        };
        let error = limits.check_frame(2, 2, 6).unwrap_err();
        let info = DecoderInfo {
            backend: BackendKind::Native,
            isolation: Isolation::InProcess,
            decoder: "terminal fixture".into(),
        };
        let dropped = Arc::new(AtomicBool::new(false));
        let mut checked = Checked {
            decoder: Some(Box::new(Live {
                info: info.clone(),
                dropped: Arc::clone(&dropped),
                frame: Some(packed_frame(1).frame),
                terminal: Some(TerminalOutput {
                    frames: [packed_frame(2)].into(),
                    error: error.clone(),
                }),
            })),
            info,
            limits,
            terminal: None,
        };
        assert_eq!(checked.receive_frame().unwrap_err(), error);
        assert!(dropped.load(Ordering::SeqCst));
        assert!(checked.decoder.is_none());
        assert!(checked.terminal.as_ref().unwrap().frames.is_empty());
        assert_eq!(checked.receive_frame().unwrap_err(), error);
    }
}
