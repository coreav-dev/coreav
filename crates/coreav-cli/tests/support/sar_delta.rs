use super::ResultBytes;
use coreav_transcode::annexb::sps_aspect_ratio_info;

type Check<T> = Result<T, String>;

#[derive(Clone, Copy)]
pub enum SpsRule {
    Exact,
    Declared,
}

fn equal<T: PartialEq + ?Sized>(before: &T, after: &T, field: &str) -> Check<()> {
    if before == after {
        Ok(())
    } else {
        Err(field.into())
    }
}

pub fn check(
    before: &ResultBytes,
    after: &ResultBytes,
    declared: Option<(u64, u64)>,
    rule: SpsRule,
) -> Check<bool> {
    equal(&before.code, &after.code, "exit code")?;
    equal(&before.stderr, &after.stderr, "stderr")?;
    if before == after {
        return Ok(false);
    }
    let Some((mut n, mut d)) = declared.filter(|(n, d)| *n > 0 && *d > 0) else {
        equal(
            &before.stdout,
            &after.stdout,
            "stdout without a source SAR declaration",
        )?;
        equal(
            &before.media,
            &after.media,
            "media without a source SAR declaration",
        )?;
        return Ok(false);
    };
    let (mut a, mut b) = (n, d);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    n /= a;
    d /= a;
    equal(&before.code, &Some(0), "exit code for SAR exception")?;
    let old = before.media.as_deref().ok_or("media before")?;
    let new = after.media.as_deref().ok_or("media after")?;
    let old_boxes = boxes(old, 0, true)?;
    let new_boxes = boxes(new, 0, true)?;
    equal(
        &old_boxes.iter().map(|b| b.tag).collect::<Vec<_>>(),
        &vec![*b"ftyp", *b"moov", *b"mdat"],
        "top-level layout",
    )?;
    equal(
        &new_boxes.iter().map(|b| b.tag).collect::<Vec<_>>(),
        &vec![*b"ftyp", *b"moov", *b"mdat"],
        "top-level layout",
    )?;
    let context = Context {
        old_mdat: old_boxes[2].offset + old_boxes[2].header,
        new_mdat: new_boxes[2].offset + new_boxes[2].header,
        mdat_len: old_boxes[2].data.len(),
        declared: (n, d),
        rule,
    };
    let growth = compare_boxes(&old_boxes, &new_boxes, "", 0, &context)?;
    equal(
        &new.len().checked_sub(old.len()),
        &Some(growth),
        "file length versus SAR signalling growth",
    )?;
    let mut expected = std::str::from_utf8(&before.stdout)
        .map_err(|_| "stdout UTF-8")?
        .to_owned();
    let old_summary: serde_json::Value =
        serde_json::from_slice(&before.stdout).map_err(|_| "stdout JSON")?;
    let new_summary: serde_json::Value =
        serde_json::from_slice(&after.stdout).map_err(|_| "stdout JSON")?;
    equal(
        &old_summary["output"]["bytes"].as_u64(),
        &Some(old.len() as u64),
        "stdout output.bytes before",
    )?;
    equal(
        &new_summary["output"]["bytes"].as_u64(),
        &Some(new.len() as u64),
        "stdout output.bytes after",
    )?;
    equal(
        &old_summary["video"]["sample_aspect"],
        &serde_json::Value::Null,
        "stdout video.sample_aspect before",
    )?;
    let aspect = format!("{n}:{d}");
    equal(
        &new_summary["video"]["sample_aspect"].as_str(),
        &Some(aspect.as_str()),
        "stdout video.sample_aspect after",
    )?;
    for (old_token, new_token) in [
        (
            "\"sample_aspect\": null".to_owned(),
            format!("\"sample_aspect\": \"{aspect}\""),
        ),
        (
            format!("\"bytes\": {}", old.len()),
            format!("\"bytes\": {}", new.len()),
        ),
    ] {
        equal(
            &expected.matches(&old_token).count(),
            &1,
            "stdout unique SAR or length token",
        )?;
        expected = expected.replacen(&old_token, &new_token, 1);
    }
    equal(
        expected.as_bytes(),
        after.stdout.as_slice(),
        "stdout outside video.sample_aspect and output.bytes",
    )?;
    Ok(true)
}

struct Context {
    old_mdat: usize,
    new_mdat: usize,
    mdat_len: usize,
    declared: (u64, u64),
    rule: SpsRule,
}

struct Atom<'a> {
    tag: [u8; 4],
    data: &'a [u8],
    offset: usize,
    header: usize,
    size: usize,
    to_end: bool,
}

fn number(data: &[u8], at: usize, bytes: usize, field: &str) -> Check<u64> {
    let end = at.checked_add(bytes).ok_or(field)?;
    let slice = data.get(at..end).ok_or(field)?;
    Ok(slice.iter().fold(0, |n, byte| n * 256 + u64::from(*byte)))
}

fn boxes(data: &[u8], base: usize, top: bool) -> Check<Vec<Atom<'_>>> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < data.len() {
        if result.len() >= 4096 {
            return Err("box count limit".into());
        }
        let size32 = number(data, at, 4, "box size")?;
        let tag = data
            .get(at + 4..at + 8)
            .ok_or("box type")?
            .try_into()
            .map_err(|_| "box type")?;
        let header = if size32 == 1 { 16 } else { 8 };
        let size = match size32 {
            0 if top => data.len() - at,
            0 => return Err("nested box size zero".into()),
            1 => usize::try_from(number(data, at + 8, 8, "extended box size")?)
                .map_err(|_| "extended box size")?,
            n => usize::try_from(n).map_err(|_| "box size")?,
        };
        let end = at
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .ok_or("box parent length")?;
        if size < header {
            return Err("box header length".into());
        }
        result.push(Atom {
            tag,
            data: &data[at + header..end],
            offset: base + at,
            header,
            size,
            to_end: size32 == 0,
        });
        at = end;
    }
    Ok(result)
}

fn compare_boxes(
    old: &[Atom<'_>],
    new: &[Atom<'_>],
    path: &str,
    depth: usize,
    context: &Context,
) -> Check<usize> {
    if depth > 12 {
        return Err("box depth limit".into());
    }
    let mut extra = None;
    let mut growth = 0;
    if path.ends_with("/avc1") {
        let indices: Vec<_> = new
            .iter()
            .enumerate()
            .filter(|(_, b)| b.tag == *b"pasp")
            .map(|(i, _)| i)
            .collect();
        if let [i] = indices.as_slice() {
            if *i == 0 || new[*i - 1].tag != *b"avcC" {
                return Err(format!("{path}/pasp order"));
            }
            let pasp = &new[*i];
            equal(&pasp.header, &8, "pasp header")?;
            equal(&pasp.data.len(), &8, "pasp length")?;
            equal(
                &(
                    number(pasp.data, 0, 4, "pasp numerator")?,
                    number(pasp.data, 4, 4, "pasp denominator")?,
                ),
                &context.declared,
                "pasp declared ratio",
            )?;
            if old.iter().all(|b| b.tag != *b"pasp") {
                growth = pasp.size;
                extra = Some(*i);
            }
        } else {
            return Err(format!("{path}/missing or duplicate pasp"));
        }
    }
    let filtered: Vec<_> = new
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != extra)
        .map(|(_, b)| b)
        .collect();
    equal(&old.len(), &filtered.len(), &format!("box order {path}"))?;
    for (old, new) in old.iter().zip(filtered) {
        equal(&old.tag, &new.tag, &format!("box order {path}"))?;
        let field = format!("{path}/{}", String::from_utf8_lossy(&old.tag));
        equal(&old.header, &new.header, &format!("{field} header form"))?;
        equal(&old.to_end, &new.to_end, &format!("{field} size form"))?;
        let prefix = match &old.tag {
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" | b"edts" | b"dinf" => Some(0),
            b"stsd" => Some(8),
            b"avc1" if path.ends_with("/stsd") => Some(78),
            _ => None,
        };
        let delta = if let Some(prefix) = prefix {
            let a = old.data.get(..prefix).ok_or(format!("{field} header"))?;
            let b = new.data.get(..prefix).ok_or(format!("{field} header"))?;
            equal(a, b, &format!("{field} header"))?;
            compare_boxes(
                &boxes(&old.data[prefix..], old.offset + old.header + prefix, false)?,
                &boxes(&new.data[prefix..], new.offset + new.header + prefix, false)?,
                &field,
                depth + 1,
                context,
            )?
        } else if old.tag == *b"avcC" && path.ends_with("/stsd/avc1") {
            compare_avcc(old.data, new.data, context)?
        } else if matches!(&old.tag, b"stco" | b"co64") && path.ends_with("/stbl") {
            equal(&old.data.len(), &new.data.len(), &format!("{field} length"))?;
            equal(
                old.data.get(..8).ok_or("chunk offset header")?,
                new.data.get(..8).ok_or("chunk offset header")?,
                &format!("{field} header"),
            )?;
            let count = usize::try_from(number(old.data, 4, 4, "chunk offset count")?)
                .map_err(|_| "chunk offset count")?;
            let width = if old.tag == *b"stco" { 4 } else { 8 };
            equal(
                &count.checked_mul(width).and_then(|n| n.checked_add(8)),
                &Some(old.data.len()),
                &format!("{field} count"),
            )?;
            for i in 0..count {
                let label = format!("{field}[{i}]");
                let a = number(old.data, 8 + i * width, width, &label)?
                    .checked_sub(context.old_mdat as u64)
                    .filter(|n| *n < context.mdat_len as u64)
                    .ok_or(&label)?;
                let b = number(new.data, 8 + i * width, width, &label)?
                    .checked_sub(context.new_mdat as u64)
                    .filter(|n| *n < context.mdat_len as u64)
                    .ok_or(&label)?;
                equal(&a, &b, &label)?;
            }
            0
        } else {
            equal(old.data, new.data, &field)?;
            0
        };
        equal(
            &old.size.checked_add(delta),
            &Some(new.size),
            &format!("{field} forced length"),
        )?;
        growth = growth.checked_add(delta).ok_or("SAR growth limit")?;
    }
    Ok(growth)
}

fn parameter<'a>(data: &'a [u8], at: &mut usize, field: &str) -> Check<&'a [u8]> {
    let size = usize::try_from(number(data, *at, 2, field)?).map_err(|_| field)?;
    *at += 2;
    let end = at.checked_add(size).ok_or(field)?;
    let value = data.get(*at..end).ok_or(field)?;
    *at = end;
    Ok(value)
}

fn compare_avcc(old: &[u8], new: &[u8], context: &Context) -> Check<usize> {
    equal(
        old.get(..6).ok_or("avcC header")?,
        new.get(..6).ok_or("avcC header")?,
        "avcC header",
    )?;
    if old[5] & 31 != 1 {
        return Err("avcC SPS count".into());
    }
    let (mut a, mut b) = (6, 6);
    let old_sps = parameter(old, &mut a, "avcC SPS length")?;
    let new_sps = parameter(new, &mut b, "avcC SPS length")?;
    if old.get(a) != Some(&1) || new.get(b) != Some(&1) {
        return Err("avcC PPS count".into());
    }
    a += 1;
    b += 1;
    equal(
        parameter(old, &mut a, "avcC PPS length")?,
        parameter(new, &mut b, "avcC PPS length")?,
        "avcC PPS",
    )?;
    equal(&old[a..], &new[b..], "avcC extensions")?;
    if old_sps == new_sps {
        return Ok(0);
    }
    if matches!(context.rule, SpsRule::Exact) {
        return Err("avcC SPS must remain exact".into());
    }
    let old_info = sps_aspect_ratio_info(old_sps)
        .map_err(|_| "avcC SPS syntax before")?
        .ok_or("avcC SPS missing VUI before")?;
    let new_info = sps_aspect_ratio_info(new_sps)
        .map_err(|_| "avcC SPS syntax after")?
        .ok_or("avcC SPS missing VUI after")?;
    equal(
        &old_info.flag_bit,
        &new_info.flag_bit,
        "avcC SPS aspect position",
    )?;
    equal(&old_info.ratio, &None, "avcC SPS prior declaration")?;
    equal(
        &new_info.ratio.map(|(n, d)| (u64::from(n), u64::from(d))),
        &Some(context.declared),
        "avcC SPS declared ratio",
    )?;
    equal(&old_sps[0], &new_sps[0], "avcC SPS NAL header")?;
    let mut old_bits = sodb(&old_sps[1..])?;
    let mut new_bits = sodb(&new_sps[1..])?;
    let p = usize::try_from(old_info.flag_bit).map_err(|_| "avcC SPS aspect position")?;
    equal(
        old_bits.get(p).ok_or("avcC SPS aspect flag")?,
        &false,
        "avcC SPS prior aspect flag",
    )?;
    equal(
        new_bits.get(p).ok_or("avcC SPS aspect flag")?,
        &true,
        "avcC SPS new aspect flag",
    )?;
    let idc = new_bits
        .get(p + 1..p + 9)
        .ok_or("avcC SPS aspect idc")?
        .iter()
        .fold(0u8, |n, bit| (n << 1) | u8::from(*bit));
    let end = p + 1 + if idc == 255 { 40 } else { 8 };
    if end > new_bits.len() {
        return Err("avcC SPS aspect length".into());
    }
    new_bits.drain(p..end);
    old_bits.remove(p);
    equal(&old_bits, &new_bits, "avcC SPS outside aspect_ratio_info")?;
    new_sps
        .len()
        .checked_sub(old_sps.len())
        .ok_or("avcC SPS growth".into())
}

fn sodb(ebsp: &[u8]) -> Check<Vec<bool>> {
    let mut rbsp = Vec::new();
    let mut zeros = 0;
    for byte in ebsp {
        if zeros == 2 && *byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(*byte);
        zeros = if *byte == 0 { zeros + 1 } else { 0 };
    }
    let mut canonical = Vec::new();
    zeros = 0;
    for byte in &rbsp {
        if zeros == 2 && *byte <= 3 {
            canonical.push(3);
            zeros = 0;
        }
        canonical.push(*byte);
        zeros = if *byte == 0 { zeros + 1 } else { 0 };
    }
    equal(ebsp, canonical.as_slice(), "avcC SPS emulation prevention")?;
    let mut bits: Vec<_> = rbsp
        .iter()
        .flat_map(|byte| (0..8).rev().map(move |i| byte & (1 << i) != 0))
        .collect();
    let stop = bits
        .iter()
        .rposition(|bit| *bit)
        .ok_or("avcC SPS rbsp_trailing_bits")?;
    if bits.len() - stop > 8 {
        return Err("avcC SPS rbsp_trailing_bits".into());
    }
    bits.truncate(stop);
    Ok(bits)
}

pub fn require_payload_negative(
    before: &ResultBytes,
    after: &ResultBytes,
    declared: Option<(u64, u64)>,
    rule: SpsRule,
) -> Check<()> {
    let mut mutated = after.clone();
    let media = mutated.media.as_mut().ok_or("negative media")?;
    let offset = {
        let atoms = boxes(media, 0, true)?;
        let mdat = atoms
            .iter()
            .find(|b| b.tag == *b"mdat")
            .ok_or("negative mdat")?;
        if mdat.data.is_empty() {
            return Err("negative empty mdat".into());
        }
        mdat.offset + mdat.header
    };
    media[offset] ^= 1;
    match check(before, &mutated, declared, rule) {
        Err(field) if field == "/mdat" => Ok(()),
        Err(field) => Err(format!("negative mdat reported {field}")),
        Ok(_) => Err("negative non-SAR mdat mutation was accepted".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn old_sps() -> Vec<u8> {
        hex("6764000aacd9462684000003000400000301e03c489658")
    }
    fn new_sps() -> Vec<u8> {
        hex("6764000aacd94626c044000003000400000301e03c489658")
    }

    fn atom(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = ((data.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(tag);
        out.extend(data);
        out
    }

    fn media(sps: &[u8], pasps: usize, extra: bool, wide: bool) -> Vec<u8> {
        let mut config = hex("0164000affe1");
        config.extend((sps.len() as u16).to_be_bytes());
        config.extend(sps);
        config.extend(hex("01000668ebe3cb22c0fdf8f800"));
        let mut entry = vec![0; 78];
        entry.extend(atom(b"avcC", &config));
        for _ in 0..pasps {
            entry.extend(atom(b"pasp", &hex("0000000100000001")));
        }
        if extra {
            entry.extend(atom(b"free", &[]));
        }
        let mut stsd = hex("0000000000000001");
        stsd.extend(atom(b"avc1", &entry));
        let ftyp = atom(b"ftyp", b"isom");
        let make_moov = |offset: u64| {
            let mut offsets = hex("0000000000000001");
            if wide {
                offsets.extend(offset.to_be_bytes());
            } else {
                offsets.extend((offset as u32).to_be_bytes());
            }
            let mut stbl = atom(b"stsd", &stsd);
            stbl.extend(atom(if wide { b"co64" } else { b"stco" }, &offsets));
            atom(
                b"moov",
                &atom(
                    b"trak",
                    &atom(b"mdia", &atom(b"minf", &atom(b"stbl", &stbl))),
                ),
            )
        };
        let moov = make_moov((ftyp.len() + make_moov(0).len() + 8) as u64);
        [ftyp, moov, atom(b"mdat", &[1, 2, 3, 4])].concat()
    }

    fn result(media: Vec<u8>, sar: Option<&str>) -> ResultBytes {
        let document = serde_json::json!({"output":{"bytes":media.len()},"video":{"sample_aspect":sar,"frames":7,"width":96},"schema":"coreav.transcode/1"});
        ResultBytes {
            code: Some(0),
            stdout: serde_json::to_vec_pretty(&document).unwrap(),
            stderr: Vec::new(),
            media: Some(media),
        }
    }

    fn pair(sps: &[u8], wide: bool) -> (ResultBytes, ResultBytes) {
        (
            result(media(&old_sps(), 0, false, wide), None),
            result(media(sps, 1, false, wide), Some("1:1")),
        )
    }

    fn tag(media: &[u8], name: &[u8; 4]) -> usize {
        media.windows(4).position(|b| b == name).unwrap()
    }

    fn aspect_sps(idc: u8, n: u16, d: u16) -> Vec<u8> {
        let source = old_sps();
        let mut bits = sodb(&source[1..]).unwrap();
        let p = sps_aspect_ratio_info(&source).unwrap().unwrap().flag_bit as usize;
        let mut aspect = vec![true];
        let fields = if idc == 255 {
            vec![(8, u64::from(idc)), (16, u64::from(n)), (16, u64::from(d))]
        } else {
            vec![(8, u64::from(idc))]
        };
        for (width, value) in fields {
            aspect.extend((0..width).rev().map(|i| value & (1 << i) != 0));
        }
        bits.splice(p..p + 1, aspect);
        bits.push(true);
        while bits.len() % 8 != 0 {
            bits.push(false);
        }
        let mut out = vec![source[0]];
        let mut zeros = 0;
        for chunk in bits.chunks_exact(8) {
            let byte = chunk.iter().fold(0u8, |n, bit| (n << 1) | u8::from(*bit));
            if zeros == 2 && byte <= 3 {
                out.push(3);
                zeros = 0;
            }
            out.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        out
    }

    #[test]
    fn sar_growth_is_structurally_exact() {
        for wide in [false, true] {
            for sps in [new_sps(), aspect_sps(255, 1, 1)] {
                let (before, after) = pair(&sps, wide);
                assert!(check(&before, &after, Some((2, 2)), SpsRule::Declared).unwrap());
                require_payload_negative(&before, &after, Some((1, 1)), SpsRule::Declared).unwrap();
            }
            let before = result(media(&new_sps(), 0, false, wide), None);
            let after = result(media(&new_sps(), 1, false, wide), Some("1:1"));
            assert_eq!(
                after.media.as_ref().unwrap().len() - before.media.as_ref().unwrap().len(),
                16
            );
            assert!(check(&before, &after, Some((1, 1)), SpsRule::Exact).unwrap());
            assert!(!check(&before, &before, None, SpsRule::Exact).unwrap());
        }
    }

    #[test]
    fn non_sar_mutations_fail_with_the_field() {
        let (before, after) = pair(&new_sps(), false);
        let mut cases = Vec::new();
        cases.push((
            "SPS declared ratio",
            result(media(&aspect_sps(2, 0, 0), 1, false, false), Some("1:1")),
        ));
        for (name, offset) in [("/mdat", 4), ("avc1 header", 4 + 24), ("stco[0]", 12 + 3)] {
            let mut changed = after.clone();
            let bytes = changed.media.as_mut().unwrap();
            let key = if name == "/mdat" {
                b"mdat"
            } else if name == "avc1 header" {
                b"avc1"
            } else {
                b"stco"
            };
            let at = tag(bytes, key) + offset;
            bytes[at] ^= 1;
            cases.push((name, changed));
        }
        let mut changed = after.clone();
        changed.code = Some(1);
        cases.push(("exit code", changed));
        let mut changed = after.clone();
        changed.stderr.push(b'x');
        cases.push(("stderr", changed));
        for (from, to, field) in [
            ("\"frames\": 7", "\"frames\": 8", "stdout outside"),
            ("\"bytes\":", "\"not_bytes\":", "stdout output.bytes"),
        ] {
            let mut changed = after.clone();
            changed.stdout = String::from_utf8(changed.stdout)
                .unwrap()
                .replace(from, to)
                .into_bytes();
            cases.push((field, changed));
        }
        let mut changed = after.clone();
        let bytes = changed.media.as_mut().unwrap();
        let at = tag(bytes, b"avcC");
        let pps = at + 4 + 6 + 2 + new_sps().len() + 1 + 2;
        bytes[pps] ^= 1;
        cases.push(("avcC PPS", changed));
        for (count, extra, label) in [
            (0, false, "pasp"),
            (2, false, "duplicate pasp"),
            (1, true, "box order"),
        ] {
            cases.push((
                label,
                result(media(&new_sps(), count, extra, false), Some("1:1")),
            ));
        }
        let mut changed = after.clone();
        let bytes = changed.media.as_mut().unwrap();
        let at = tag(bytes, b"moov") - 1;
        bytes[at] += 1;
        cases.push(("box", changed));
        let mut changed = after.clone();
        changed.media.as_mut().unwrap().extend(atom(b"mdat", &[5]));
        cases.push(("top-level layout", changed));
        for (index, mask, field) in [(3, 1, "avcC SPS outside"), (23, 8, "avcC SPS outside")] {
            let mut sps = new_sps();
            sps[index] ^= mask;
            cases.push((field, result(media(&sps, 1, false, false), Some("1:1"))));
        }
        let mut sps = new_sps();
        let at = sps.windows(3).position(|b| b == [0, 0, 3]).unwrap() + 2;
        sps.remove(at);
        cases.push((
            "emulation prevention",
            result(media(&sps, 1, false, false), Some("1:1")),
        ));
        cases.push((
            "SPS syntax",
            result(media(&new_sps()[..3], 1, false, false), Some("1:1")),
        ));
        for (field, mutated) in cases {
            let error = check(&before, &mutated, Some((1, 1)), SpsRule::Declared).unwrap_err();
            assert!(error.contains(field), "expected {field}, got {error}");
        }
        assert!(
            check(&before, &after, None, SpsRule::Declared)
                .unwrap_err()
                .contains("source SAR")
        );
        assert!(
            check(&before, &after, Some((2, 1)), SpsRule::Declared)
                .unwrap_err()
                .contains("declared ratio")
        );
        assert!(
            check(&before, &after, Some((1, 1)), SpsRule::Exact)
                .unwrap_err()
                .contains("SPS must remain exact")
        );
    }

    #[test]
    fn co64_offset_mutation_is_not_a_forced_offset() {
        let (before, mut after) = pair(&new_sps(), true);
        let bytes = after.media.as_mut().unwrap();
        let at = tag(bytes, b"co64") + 12 + 7;
        bytes[at] ^= 1;
        assert!(
            check(&before, &after, Some((1, 1)), SpsRule::Declared)
                .unwrap_err()
                .contains("co64[0]")
        );
    }
}
