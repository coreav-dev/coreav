use crate::{Result, common};
use coreav_hevc::NalHeader;

pub(crate) fn units(
    reader: impl std::io::Read,
    maximum: u64,
    mut emit: impl FnMut(&[u8], Option<i64>, Option<u8>, &[Vec<u8>]) -> Result<()>,
) -> Result<(u64, u64)> {
    let (mut packet, mut parameters) = (Vec::new(), Vec::new());
    let mut parameter_bytes = 0usize;
    let (mut has_vcl, mut pts, mut kind) = (false, None, None);
    let (mut coded, mut packets) = (0i64, 0u64);
    let mut failure = None;
    let receipt = common::read_nals(
        reader,
        coreav_hevc::Limits::default().max_nal_bytes,
        |_, nal| {
            let header = NalHeader::parse(nal)?;
            let first = header.unit_type < 32 && nal.get(2).is_some_and(|byte| byte & 128 != 0);
            let prefix = matches!(header.unit_type, 32..=35 | 39 | 41..=44 | 48..=55);
            if has_vcl && (prefix || first) {
                emit(&packet, pts, kind, &parameters).map_err(|error| {
                    failure = Some(error);
                    coreav_hevc::Error::InvalidData("boundary_access_unit")
                })?;
                packets += 1;
                packet.clear();
                has_vcl = false;
                pts = None;
                kind = None;
            }
            if (32..=34).contains(&header.unit_type) {
                let mut owned = Vec::new();
                parameter_bytes = parameter_bytes
                    .checked_add(nal.len())
                    .filter(|n| *n <= 64 << 20)
                    .ok_or(coreav_hevc::Error::Limit("parameter_replay_bytes"))?;
                owned
                    .try_reserve_exact(nal.len())
                    .map_err(|_| coreav_hevc::Error::Limit("parameter_replay_allocation"))?;
                owned.extend_from_slice(nal);
                parameters
                    .try_reserve(1)
                    .map_err(|_| coreav_hevc::Error::Limit("parameter_replay_allocation"))?;
                parameters.push(owned);
            }
            if first {
                pts = Some(coded);
                kind = Some(header.unit_type);
                coded = coded
                    .checked_add(1)
                    .ok_or(coreav_hevc::Error::Limit("picture_count"))?;
            }
            has_vcl |= header.unit_type < 32;
            let wanted = packet
                .len()
                .checked_add(4)
                .and_then(|n| n.checked_add(nal.len()))
                .filter(|n| *n as u64 <= maximum)
                .ok_or(coreav_hevc::Error::Limit("access_unit_bytes"))?;
            packet
                .try_reserve_exact(wanted - packet.len())
                .map_err(|_| coreav_hevc::Error::Limit("access_unit_allocation"))?;
            if packet.capacity() as u64 > maximum {
                return Err(coreav_hevc::Error::Limit("access_unit_capacity"));
            }
            packet.extend_from_slice(&[0, 0, 0, 1]);
            packet.extend_from_slice(nal);
            Ok(())
        },
    );
    let receipt = match receipt {
        Ok(receipt) => receipt,
        Err(error) => return Err(failure.unwrap_or_else(|| error.into())),
    };
    if !packet.is_empty() {
        emit(&packet, pts, kind, &parameters)?;
        packets += 1;
    }
    Ok((receipt.bytes, packets))
}
