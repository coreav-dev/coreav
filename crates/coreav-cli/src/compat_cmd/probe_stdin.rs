use std::io::Write;
use std::process::ExitCode;

use coreav_cli::error::Failure;
use coreav_cli::fallback::{NotNative, Tool};
use coreav_cli::ffprobe::{self, args::Options};
use coreav_cli::strict_safe::StrictSafe;
use coreav_isobmff::{BufferedSource, FileSource, Limits, Parsed, Source, parse_box_header};

use super::{EXIT_IO, EncodedInput, fallback_cmd, message};

pub(super) fn run(args: &[String], options: &Options, strict_safe: StrictSafe) -> ExitCode {
    let input = match EncodedInput::read() {
        Ok(input) => input,
        Err(error) => {
            message(
                "ffprobe",
                &format!("could not acquire encoded stdin: {error}"),
            );
            return ExitCode::from(EXIT_IO);
        }
    };
    match render(&input, options) {
        Ok(bytes) => {
            let mut out = std::io::stdout().lock();
            match out.write_all(&bytes).and_then(|()| out.flush()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => super::super::write_error(&error),
            }
        }
        Err(_) => {
            let refusal = NotNative::property(
                "standard input facts",
                "structured stdin requires a qualified front-loaded ISO movie",
            );
            match input.into_replay() {
                Ok((file, input)) => fallback_cmd::refuse_with_input(
                    Tool::Ffprobe,
                    args,
                    refusal,
                    strict_safe,
                    file,
                    input,
                ),
                Err(error) => {
                    message(
                        "ffprobe",
                        &format!("could not replay encoded stdin: {error}"),
                    );
                    ExitCode::from(EXIT_IO)
                }
            }
        }
    }
}

fn render(input: &EncodedInput, options: &Options) -> Result<Vec<u8>, Failure> {
    let mut source = FileSource::open(&input.path).map_err(Failure::library)?;
    let size = source.len().map_err(Failure::library)?;
    let limits = Limits::default();
    front_loaded(&mut source, size, &limits)?;
    let mut source = BufferedSource::new(source);
    let mut probed =
        ffprobe::probe_limited(&mut source, size, &options.input, options.force, &limits)?;
    probed
        .report
        .format
        .retain(|(key, _)| !matches!(*key, "size" | "bit_rate"));
    let mut output = Staged {
        bytes: Vec::new(),
        limit: usize::try_from(limits.max_retained_bytes).map_err(|_| unsupported())?,
    };
    ffprobe::emit(options, &mut probed, &mut output)?;
    Ok(output.bytes)
}

fn unsupported() -> Failure {
    Failure::Unsupported("unqualified stdin layout or exhausted staging limit".to_owned())
}

fn front_loaded<S: Source>(source: &mut S, size: u64, limits: &Limits) -> Result<(), Failure> {
    let mut at = 0u64;
    let mut boxes = 0u32;
    let (mut movie, mut media) = (false, false);
    while at < size {
        boxes += 1;
        if boxes > limits.max_top_level_boxes {
            return Err(unsupported());
        }
        let room = size.checked_sub(at).ok_or_else(unsupported)?;
        let mut bytes = [0u8; coreav_isobmff::MAX_HEADER_LEN];
        let count = usize::try_from(room.min(bytes.len() as u64)).map_err(|_| unsupported())?;
        let header = bytes.get_mut(..count).ok_or_else(unsupported)?;
        source.read_exact_at(at, header).map_err(Failure::library)?;
        let Parsed::Done(header) = parse_box_header(header, at, room).map_err(Failure::library)?
        else {
            return Err(unsupported());
        };
        match &header.kind.0 {
            b"moov" if !movie && !media => movie = true,
            b"mdat" if movie => media = true,
            b"ftyp" | b"free" | b"skip" | b"wide" => {}
            _ => return Err(unsupported()),
        }
        at = header.end();
    }
    if movie && media {
        Ok(())
    } else {
        Err(unsupported())
    }
}

struct Staged {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for Staged {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let end = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|end| *end <= self.limit)
            .ok_or_else(|| std::io::Error::other("structured stdin staging limit"))?;
        self.bytes
            .try_reserve_exact(end - self.bytes.len())
            .map_err(std::io::Error::other)?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use coreav_isobmff::SliceSource;

    fn atom(kind: &[u8; 4]) -> Vec<u8> {
        [8u32.to_be_bytes().as_slice(), kind].concat()
    }

    #[test]
    fn only_complete_front_loaded_nonfragmented_iso_layouts_are_qualified() {
        let valid = [atom(b"ftyp"), atom(b"moov"), atom(b"mdat")].concat();
        let check = |bytes: &[u8], limits: Limits| {
            front_loaded(&mut SliceSource::new(bytes), bytes.len() as u64, &limits)
        };
        assert!(check(&valid, Limits::default()).is_ok());
        for bytes in [
            [atom(b"mdat"), atom(b"moov")].concat(),
            [atom(b"moov"), atom(b"moof"), atom(b"mdat")].concat(),
            [atom(b"moov"), atom(b"mdat"), atom(b"moov")].concat(),
            [atom(b"moov"), atom(b"mdat"), atom(b"uuid")].concat(),
            atom(b"moov"),
            atom(b"mdat"),
            Vec::new(),
            valid[..valid.len() - 1].to_vec(),
        ] {
            assert!(check(&bytes, Limits::default()).is_err());
        }
        assert!(
            check(
                &valid,
                Limits {
                    max_top_level_boxes: 2,
                    ..Limits::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn staging_limit_fails_without_committing_a_partial_chunk() {
        let mut output = Staged {
            bytes: Vec::new(),
            limit: 8,
        };
        output.write_all(b"12345678").unwrap();
        assert!(output.write_all(b"9").is_err());
        assert_eq!(output.bytes, b"12345678");
        assert!(output.flush().is_ok());
    }
}
