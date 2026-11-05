use std::io::{Read, Write};
use std::process::ExitCode;

use coreav_cli::error::Failure;
use coreav_cli::fallback::{NotNative, Tool};
use coreav_cli::ffprobe::{self, args::Options};
use coreav_cli::strict_safe::StrictSafe;
use coreav_isobmff::{
    BufferedSource, FileSource, Limits, Parsed, SliceSource, Source, parse_box_header,
};

use super::{EXIT_IO, EncodedInput, fallback_cmd, message};

const PREFIX_LIMIT: usize = 1_048_576;

pub(super) fn run(args: &[String], options: &Options, strict_safe: StrictSafe) -> ExitCode {
    let acquired = prefix(&mut std::io::stdin().lock());
    let input = match acquired {
        Ok(Acquired::Complete(bytes)) => {
            EncodedInput::read_from(&mut std::io::Cursor::new(bytes), 1_073_741_824)
        }
        Ok(Acquired::Live(bytes)) => {
            return fallback_cmd::refuse_with_prefix(
                Tool::Ffprobe,
                args,
                refusal(),
                strict_safe,
                bytes,
            );
        }
        Err(error) => Err(error),
    };
    let input = match input {
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
        Err(_) => match input.into_replay() {
            Ok((file, input)) => fallback_cmd::refuse_with_input(
                Tool::Ffprobe,
                args,
                refusal(),
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
        },
    }
}

fn refusal() -> NotNative {
    NotNative::property(
        "standard input facts",
        "structured stdin requires a complete qualified ISO movie within the prefix limit",
    )
}

enum Acquired {
    Complete(Vec<u8>),
    Live(Vec<u8>),
}

fn prefix(reader: &mut dyn Read) -> std::io::Result<Acquired> {
    let limits = Limits::default();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 65_536];
    loop {
        let room = (PREFIX_LIMIT - bytes.len()).min(buffer.len());
        let chunk = buffer
            .get_mut(..room)
            .ok_or_else(|| std::io::Error::other("stdin prefix limit"))?;
        let count = match reader.read(chunk) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(Acquired::Complete(bytes));
        }
        bytes.try_reserve(count).map_err(std::io::Error::other)?;
        bytes.extend_from_slice(
            buffer
                .get(..count)
                .ok_or_else(|| std::io::Error::other("stdin prefix read"))?,
        );
        if bytes.len() == PREFIX_LIMIT
            || front_loaded(
                &mut SliceSource::new(&bytes),
                bytes.len() as u64,
                &limits,
                false,
            )
            .is_err()
        {
            return Ok(Acquired::Live(bytes));
        }
    }
}

fn render(input: &EncodedInput, options: &Options) -> Result<Vec<u8>, Failure> {
    let mut source = FileSource::open(&input.path).map_err(Failure::library)?;
    let size = source.len().map_err(Failure::library)?;
    let limits = Limits::default();
    front_loaded(&mut source, size, &limits, true)?;
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

fn front_loaded<S: Source>(
    source: &mut S,
    size: u64,
    limits: &Limits,
    complete: bool,
) -> Result<(), Failure> {
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
        let extent = if complete { room } else { u64::MAX - at };
        let Parsed::Done(header) =
            parse_box_header(header, at, extent).map_err(Failure::library)?
        else {
            return if complete { Err(unsupported()) } else { Ok(()) };
        };
        match &header.kind.0 {
            b"moov" if !movie && !media => movie = true,
            b"mdat" if movie => media = true,
            b"ftyp" | b"free" | b"skip" | b"wide" => {}
            _ => return Err(unsupported()),
        }
        if !complete && header.end() >= PREFIX_LIMIT as u64 {
            return Err(unsupported());
        }
        if header.end() > size && !complete {
            return Ok(());
        }
        at = header.end();
    }
    if !complete || movie && media {
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
            .try_reserve(end - self.bytes.len())
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
            front_loaded(
                &mut SliceSource::new(bytes),
                bytes.len() as u64,
                &limits,
                true,
            )
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
    fn prefix_refuses_early_and_bounds_an_unending_large_movie() {
        struct Live(bool);
        impl Read for Live {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                assert!(!self.0);
                self.0 = true;
                bytes.get_mut(..9).unwrap().copy_from_slice(b"not media");
                Ok(9)
            }
        }
        let Acquired::Live(bytes) = prefix(&mut Live(false)).unwrap() else {
            panic!("unadmitted live content must not wait for EOF");
        };
        assert_eq!(bytes, b"not media");
        let header = [
            atom(b"ftyp"),
            atom(b"moov"),
            [2_147_483_648u32.to_be_bytes().as_slice(), b"mdat"].concat(),
        ]
        .concat();
        let mut reader = std::io::Cursor::new(&header).chain(std::io::repeat(0));
        let Acquired::Live(bytes) = prefix(&mut reader).unwrap() else {
            panic!("a large live movie must reach fallback before EOF");
        };
        assert_eq!(bytes, header);
        let mut reader = std::io::repeat(0);
        let prefix = [1_000_000u32.to_be_bytes().as_slice(), b"free"].concat();
        let mut reader = std::io::Cursor::new(prefix).chain(&mut reader);
        let Acquired::Live(bytes) = super::prefix(&mut reader).unwrap() else {
            panic!("an undecidable prefix must reach fallback at its cap");
        };
        assert_eq!(bytes.len(), PREFIX_LIMIT);
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
