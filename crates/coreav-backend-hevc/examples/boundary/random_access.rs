use super::*;
use std::collections::BTreeMap;

pub(super) fn units(
    path: &std::ffi::OsStr,
    maximum: u64,
    mut emit: impl FnMut(&[u8], Option<i64>, Option<u8>, &[Vec<u8>]) -> Result<()>,
) -> Result<(u64, u64)> {
    let (mut packet, mut parameters) = (Vec::new(), Vec::new());
    let mut parameter_bytes = 0usize;
    let (mut has_vcl, mut pts, mut kind) = (false, None, None);
    let (mut coded, mut packets) = (0i64, 0u64);
    let mut failure = None;
    let receipt = common::read_nals(
        std::fs::File::open(path)?,
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

struct Seek {
    decoder: Box<dyn Decoder>,
    expected: BTreeMap<i64, Value>,
    seen: Vec<Value>,
    start: i64,
    kind: u8,
    following: usize,
}

impl Seek {
    fn drain(&mut self) -> Result<()> {
        while let Some(frame) = self.decoder.receive_frame()? {
            self.seen.push(frame_record(&frame)?.0);
        }
        Ok(())
    }

    fn feed(&mut self, data: &[u8], pts: Option<i64>) -> Result<()> {
        self.decoder.send_packet(&Packet {
            data,
            pts,
            dts: None,
            keyframe: pts == Some(self.start),
            discard: !pts.is_some_and(|pts| self.expected.contains_key(&pts)),
        })?;
        self.drain()
    }

    fn finish(mut self, full: &[Value]) -> Result<Value> {
        self.decoder.send_eof()?;
        self.drain()?;
        let expected: Vec<&Value> = full
            .iter()
            .filter(|row| {
                row.get("timestamp")
                    .and_then(Value::as_i64)
                    .is_some_and(|pts| self.expected.contains_key(&pts))
            })
            .collect();
        if self.seen.len() != expected.len()
            || self.seen.iter().zip(&expected).any(|(a, b)| a != *b)
        {
            return Err("C5 native seek differs from full decode".into());
        }
        Ok(
            json!({"start":self.start,"unit_type":self.kind,"matched_outputs":expected.len(),
            "following_packets":self.following,"keyframe_available":self.expected.contains_key(&self.start)}),
        )
    }
}

pub(super) fn check(path: &std::ffi::OsStr, limits: &Limits, full: &[Value]) -> Result<Value> {
    let mut references = BTreeMap::new();
    for row in full {
        let pts = row
            .get("timestamp")
            .and_then(Value::as_i64)
            .ok_or("C5 coded clock is missing")?;
        if references.insert(pts, row).is_some() {
            return Err("C5 coded clock is not unique".into());
        }
    }
    let mut seek: Option<Seek> = None;
    let mut results = Vec::new();
    let mut keyframes = 0usize;
    units(
        path,
        limits.max_packet_bytes,
        |data, pts, kind, parameters| {
            if kind.is_some_and(|kind| (16..=21).contains(&kind)) {
                if let Some(previous) = seek.take() {
                    results.push(previous.finish(full)?);
                }
                let start = pts.ok_or("C5 original coded identity is absent")?;
                let mut decoder =
                    NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), limits)?;
                for nal in parameters {
                    let mut framed = Vec::new();
                    framed.try_reserve_exact(4 + nal.len())?;
                    framed.extend_from_slice(&[0, 0, 0, 1]);
                    framed.extend_from_slice(nal);
                    decoder.send_packet(&Packet {
                        data: &framed,
                        pts: None,
                        dts: None,
                        keyframe: false,
                        discard: true,
                    })?;
                    if decoder.receive_frame()?.is_some() {
                        return Err("C5 parameter replay produced pixels".into());
                    }
                }
                if let Some(reference) = references.get(&start) {
                    decoder.send_packet(&Packet {
                        data,
                        pts,
                        dts: None,
                        keyframe: true,
                        discard: false,
                    })?;
                    decoder.send_eof()?;
                    let mut single = Vec::new();
                    while let Some(frame) = decoder.receive_frame()? {
                        single.push(frame_record(&frame)?.0);
                    }
                    if single.len() != 1 || single.first() != Some(*reference) {
                        return Err("C5 keyframe-only decode differs from full decode".into());
                    }
                    keyframes += 1;
                    decoder = NativeHevc::new().open(&StreamConfig::new(Codec::Hevc), limits)?;
                    for nal in parameters {
                        let mut framed = Vec::new();
                        framed.try_reserve_exact(4 + nal.len())?;
                        framed.extend_from_slice(&[0, 0, 0, 1]);
                        framed.extend_from_slice(nal);
                        decoder.send_packet(&Packet {
                            data: &framed,
                            pts: None,
                            dts: None,
                            keyframe: false,
                            discard: true,
                        })?;
                    }
                }
                seek = Some(Seek {
                    decoder,
                    expected: BTreeMap::new(),
                    seen: Vec::new(),
                    start,
                    kind: kind.ok_or("C5 IRAP kind is absent")?,
                    following: 0,
                });
            }
            if let Some(active) = &mut seek {
                if !kind.is_some_and(|kind| matches!(kind, 8 | 9)) {
                    if let Some((pts, reference)) =
                        pts.and_then(|pts| references.get(&pts).map(|row| (pts, *row)))
                    {
                        active.expected.insert(pts, reference.clone());
                    }
                }
                active.feed(data, pts)?;
                if pts != Some(active.start) {
                    active.following += 1;
                }
                if active.following == 4 {
                    let finished = seek.take().ok_or("C5 seek state is absent")?;
                    results.push(finished.finish(full)?);
                }
            }
            Ok(())
        },
    )?;
    if let Some(last) = seek {
        results.push(last.finish(full)?);
    }
    Ok(
        json!({"complete":true,"starts":results.len(),"keyframe_only_matches":keyframes,
        "matched_seek_outputs":results.iter().filter_map(|row|row.get("matched_outputs").and_then(Value::as_u64)).sum::<u64>(),
        "results":results}),
    )
}
