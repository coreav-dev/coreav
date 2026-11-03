//! Private stdio encoder entry, launched under the existing process sandbox.

use std::io::{Read, Write};

use crate::{
    Error,
    model::{self, Retention},
    native::{self, Session},
    protocol::{self, InputReader, Limits, OutputWriter, Request},
};

/// Computes the same retained layout as this executable's worker, without initializing SVT.
pub fn retention(request: Request) -> Result<Retention, Error> {
    native::retention(request.settings, request.limits.retained_bytes())
}

/// Runs one stdio job. The CLI calls this only from its sandboxed private worker entry.
pub fn run() -> Result<(), Error> {
    let input = std::io::stdin();
    let output = std::io::stdout();
    let mut output = output.lock();
    let result = encode(input.lock(), &mut output);
    let flushed = output.flush().map_err(Error::from);
    result.and(flushed)
}

fn encode<R: Read, W: Write>(input: R, output: &mut W) -> Result<(), Error> {
    let mut input = match InputReader::new(input) {
        Ok(input) => input,
        Err(error) => {
            let limits = Limits::new(model::RETAINED_BYTES, 64, 1, 1)?;
            let _ = protocol::write_failure(output, &error, limits);
            return Err(error);
        }
    };
    let request = input.request();
    let initialized = retention(request).and_then(|plan| native::version().map(|v| (plan, v)));
    let (plan, version) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            let _ = protocol::write_failure(output, &error, request.limits);
            return Err(error);
        }
    };
    let mut output = match OutputWriter::new(&mut *output, &version, request.limits) {
        Ok(output) => output,
        Err(error) => {
            if !matches!(error, Error::Io(_)) {
                let _ = protocol::write_failure(output, &error, request.limits);
            }
            return Err(error);
        }
    };
    let result = encode_request(&mut input, &mut output, plan);
    if let Err(error) = &result {
        let _ = output.failure(error);
    }
    result
}

fn encode_request<R: Read, W: Write>(
    input: &mut InputReader<R>,
    output: &mut OutputWriter<W>,
    plan: Retention,
) -> Result<(), Error> {
    let request = input.request();
    let mut session: Option<Session> = None;
    let mut base = 0;
    let mut frames = 0;
    let mut segments = 0;
    let mut packets = 0;
    while let Some(header) = input.next_frame_header()? {
        if session
            .as_ref()
            .is_some_and(|s| s.submitted() == plan.pictures())
        {
            finish_segment(session.take(), output, base, &mut packets)?;
            base = frames;
            segments += 1;
        }
        if session.is_none() {
            let next = Session::new(request.settings, request.limits.retained_bytes())?;
            if next.plan() != plan {
                return Err(Error::Output(
                    "AV1 retained layout changed between sessions",
                ));
            }
            session = Some(next);
            output.begin(base, plan, request.settings.keyint())?;
        }
        if header.id != frames {
            return Err(Error::Output("AV1 global input identity changed"));
        }
        let picture = input.read_picture()?;
        let current = session
            .as_mut()
            .ok_or(Error::Invalid("AV1 session is missing"))?;
        current.submit(picture)?;
        frames = frames
            .checked_add(1)
            .filter(|value| *value <= request.limits.frames())
            .ok_or(Error::Limit("AV1 job picture limit exceeded"))?;
        receive(current, output, &mut packets)?;
    }
    finish_segment(session.take(), output, base, &mut packets)?;
    segments += 1;
    output.complete(frames, segments)
}

fn receive<W: Write>(
    session: &mut Session,
    output: &mut OutputWriter<W>,
    packets: &mut u64,
) -> Result<(), Error> {
    while let Some(packet) = session.receive(output.packet_capacity()?)? {
        *packets = packets
            .checked_add(1)
            .filter(|value| *value <= 4 * u64::from(session.plan().pictures()) + 32)
            .ok_or(Error::Limit("AV1 segment packet limit exceeded"))?;
        output.packet(
            packet.pts,
            packet.dts,
            packet.flags,
            packet.picture_type,
            &packet.data,
        )?;
        if session.ended() {
            break;
        }
    }
    Ok(())
}

fn finish_segment<W: Write>(
    session: Option<Session>,
    output: &mut OutputWriter<W>,
    base: u64,
    packets: &mut u64,
) -> Result<(), Error> {
    let mut session = session.ok_or(Error::Output("AV1 input contains no pictures"))?;
    session.end_input()?;
    receive(&mut session, output, packets)?;
    if !session.ended() {
        return Err(Error::Output("AV1 native drain did not reach EOS"));
    }
    let submitted = session.submitted();
    let peak = session.retained_bytes()?;
    session.destroy()?;
    output.end(base, submitted, *packets, peak)?;
    *packets = 0;
    Ok(())
}
