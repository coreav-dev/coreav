use coreav_hevc::{Error, Result};
use serde_json::json;
use std::io::Write;
mod common;

fn run() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or(Error::InvalidData("syntax_input"))?;
    let source = std::fs::File::open(path).map_err(|_| Error::InvalidData("syntax_input"))?;
    let mut sets = [[None; 64]; 64];
    let mut tools = [false; 2];
    let mut slices = 0u64;
    common::read_nals(source, 64 << 20, |_, nal| {
        if let Some((layer, id, flags)) = coreav_hevc::census_tool_prefix(nal)? {
            let slot = sets
                .get_mut(usize::from(layer))
                .and_then(|sets| sets.get_mut(id as usize))
                .ok_or(Error::InvalidData("syntax_parameter_slot"))?;
            if let Some(flags) = flags {
                *slot = Some(flags);
            } else {
                let (wpp, tiles) = slot.ok_or(Error::InvalidData("syntax_missing_pps"))?;
                tools[0] |= wpp;
                tools[1] |= tiles;
                slices += 1;
            }
        }
        Ok(())
    })?;
    if slices == 0 {
        return Err(Error::InvalidData("syntax_no_slices"));
    }
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    serde_json::to_writer(
        &mut output,
        &json!({"kind":"summary","complete":true,
        "slices":slices,"tools":{"wpp":tools[0],"tiles":tools[1]}}),
    )
    .map_err(|_| Error::Limit("syntax_output"))?;
    output.flush().map_err(|_| Error::Limit("syntax_output"))
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            std::process::ExitCode::FAILURE
        }
    }
}
