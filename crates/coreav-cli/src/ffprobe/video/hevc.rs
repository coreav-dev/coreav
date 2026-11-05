use super::{Bits, HevcSpsFacts, hevc_sps_read, strict_rbsp, u16_at};

pub(crate) struct HevcConfig {
    pps_id: u32,
}

fn header(nal: &[u8]) -> Option<u8> {
    let first = *nal.first()?;
    let second = *nal.get(1)?;
    if first & 0x81 != 0 || second != 1 || nal.len() < 3 {
        return None;
    }
    Some((first >> 1) & 63)
}

pub(crate) fn summary_config(record: &[u8]) -> Option<HevcConfig> {
    if record.first() != Some(&1) || record.get(21)? & 3 != 3 {
        return None;
    }
    let (mut vps_nal, mut sps_nal, mut pps_nal) = (None, None, None);
    let mut at = 23usize;
    for _ in 0..*record.get(22)? {
        let flags = *record.get(at)?;
        if flags & 0x40 != 0 {
            return None;
        }
        let kind = flags & 63;
        if !matches!(kind, 32 | 33 | 34 | 39) {
            return None;
        }
        let count = u16_at(record, at.checked_add(1)?)?;
        at = at.checked_add(3)?;
        for _ in 0..count {
            let length = u16_at(record, at)?;
            at = at.checked_add(2)?;
            let end = at.checked_add(length)?;
            let nal = record.get(at..end)?;
            if header(nal)? != kind {
                return None;
            }
            let slot = match kind {
                32 => &mut vps_nal,
                33 => &mut sps_nal,
                34 => &mut pps_nal,
                39 => {
                    sei(nal)?;
                    at = end;
                    continue;
                }
                _ => return None,
            };
            if slot.replace(nal).is_some() {
                return None;
            }
            at = end;
        }
    }
    if at != record.len() {
        return None;
    }
    let profile = record.get(1..13)?;
    let (vps_id, ordering) = vps(vps_nal?, profile)?;
    let sps_nal = sps_nal?;
    let sps_data = strict_rbsp(sps_nal, 2)?;
    if sps_data.first()? & 15 != 1 || sps_data.get(1..13)? != profile {
        return None;
    }
    let (info, facts) = hevc_sps_read(sps_nal, false)?;
    if !facts.complete
        || !facts.progressive
        || facts.vps != vps_id
        || facts.ordering != ordering
        || facts.id > 15
        || facts.chroma_depth.checked_add(8) != Some(facts.luma_depth)
        || !matches!((*profile.first()?, facts.luma_depth), (1, 8) | (2, 8 | 10))
        || !matches!(info.pix_fmt.as_deref(), Some("yuv420p" | "yuv420p10le"))
        || !(3..=6).contains(&facts.min_cb_log2)
        || facts.min_cb_log2.checked_add(facts.cb_log2_diff)? > 6
    {
        return None;
    }
    Some(HevcConfig {
        pps_id: pps(pps_nal?, &facts)?,
    })
}

fn vps(nal: &[u8], profile: &[u8]) -> Option<(u32, [u32; 3])> {
    let data = strict_rbsp(nal, 2)?;
    if data.get(4..16)? != profile {
        return None;
    }
    let mut b = Bits::new(&data);
    let id = b.bits(4)?;
    if b.bits(2)? != 3 || b.bits(6)? != 0 || b.bits(4)? != 1 || b.bits(16)? != 65535 {
        return None;
    }
    b.bits(32)?;
    b.bits(32)?;
    b.bits(32)?;
    b.bit()?;
    let ordering = [b.ue()?, b.ue()?, b.ue()?];
    if ordering.first()? > &15 || ordering.get(1)? > ordering.first()? {
        return None;
    }
    if b.bits(6)? != 0 || b.ue()? != 0 || b.flag()? || b.flag()? || !b.trailing() {
        return None;
    }
    Some((id, ordering))
}

fn pps(nal: &[u8], sps: &HevcSpsFacts) -> Option<u32> {
    let data = strict_rbsp(nal, 2)?;
    let mut b = Bits::new(&data);
    let id = b.ue()?;
    if id > 63 || b.ue()? != sps.id {
        return None;
    }
    b.bits(2)?;
    if b.bits(3)? != 0 {
        return None;
    }
    b.bits(2)?;
    if b.ue()? > 14
        || b.ue()? > 14
        || !(-(26 + 6 * i64::from(sps.luma_depth.checked_sub(8)?))..=25).contains(&b.se()?)
    {
        return None;
    }
    b.bits(2)?;
    if b.flag()? && b.ue()? > sps.cb_log2_diff {
        return None;
    }
    if !(-12..=12).contains(&b.se()?) || !(-12..=12).contains(&b.se()?) {
        return None;
    }
    b.bits(4)?;
    if b.flag()? {
        return None;
    }
    b.bits(2)?;
    if b.flag()? {
        b.bit()?;
        if !b.flag()? && (!(-6..=6).contains(&b.se()?) || !(-6..=6).contains(&b.se()?)) {
            return None;
        }
    }
    if b.flag()? {
        return None;
    }
    b.bit()?;
    if b.ue()?
        > sps
            .min_cb_log2
            .checked_add(sps.cb_log2_diff)?
            .checked_sub(2)?
        || b.flag()?
        || b.flag()?
        || !b.trailing()
    {
        return None;
    }
    Some(id)
}

fn sei(nal: &[u8]) -> Option<()> {
    let data = strict_rbsp(nal, 2)?;
    let mut bytes = data.as_slice();
    let mut messages = false;
    while bytes != [0x80] {
        let mut value = || -> Option<usize> {
            let mut value = 0usize;
            loop {
                let (&byte, rest) = bytes.split_first()?;
                bytes = rest;
                value = value.checked_add(usize::from(byte))?;
                if byte != 255 {
                    return Some(value);
                }
            }
        };
        if value()? != 5 {
            return None;
        }
        let size = value()?;
        if size < 16 {
            return None;
        }
        bytes = bytes.get(size..)?;
        messages = true;
    }
    messages.then_some(())
}

pub(crate) fn slice_selects(config: &HevcConfig, prefix: &[u8]) -> Option<bool> {
    let kind = header(prefix)?;
    if !matches!(kind, 0..=1 | 6..=9 | 16..=21) {
        return None;
    }
    let data = super::rbsp(prefix, 2);
    let mut b = Bits::new(&data);
    if !b.flag()? {
        return None;
    }
    let irap = matches!(kind, 16..=21);
    if irap {
        b.bit()?;
    }
    (b.ue()? == config.pps_id).then_some(irap)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
pub(super) mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn sets() -> Vec<(u8, Vec<u8>)> {
        vec![
            (32, hex("40010c01ffff01600000030090000003000003001e959809")),
            (
                33,
                hex(
                    "42010101600000030090000003000003001ea02083165959ae4caf01680800001f480003a98040",
                ),
            ),
            (34, hex("4401c173d089")),
        ]
    }

    fn record(sets: &[(u8, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![0; 23];
        out[0] = 1;
        out[1..13].copy_from_slice(&hex("01600000009000000000001e"));
        out[21] = 255;
        out[22] = sets.len() as u8;
        for (kind, nal) in sets {
            out.push(*kind);
            out.extend(1u16.to_be_bytes());
            out.extend((nal.len() as u16).to_be_bytes());
            out.extend(nal);
        }
        out
    }

    pub(crate) fn fixture_record() -> Vec<u8> {
        record(&sets())
    }

    fn escaped(kind: u8, data: &[u8]) -> Vec<u8> {
        let mut out = vec![kind << 1, 1];
        let mut zeros = 0;
        for &byte in data {
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
    fn complete_linked_sets_and_only_user_data_sei_qualify() {
        let config = summary_config(&fixture_record()).unwrap();
        assert_eq!(config.pps_id, 0);
        let mut good = sets();
        let mut payload = vec![5, 16];
        payload.extend([0x55; 16]);
        payload.push(0x80);
        good.push((39, escaped(39, &payload)));
        assert!(summary_config(&record(&good)).is_some());
        for payload in [
            vec![137, 1, 0, 0x80],
            vec![4, 1, 0, 0x80],
            vec![5, 16, 0x80],
            vec![5, 1, 0, 0x80],
            vec![0x80],
        ] {
            good[3].1 = escaped(39, &payload);
            assert!(summary_config(&record(&good)).is_none());
        }
    }

    #[test]
    fn every_parameter_truncation_and_ambiguous_record_refuses() {
        let good = sets();
        let config = record(&good);
        for end in 0..config.len() {
            assert!(summary_config(&config[..end]).is_none(), "record {end}");
        }
        for index in 0..3 {
            for end in 0..good[index].1.len() {
                let mut bad = good.clone();
                bad[index].1.truncate(end);
                assert!(
                    summary_config(&record(&bad)).is_none(),
                    "set {index} cut {end}"
                );
            }
            let mut bad = good.clone();
            bad.push(bad[index].clone());
            assert!(summary_config(&record(&bad)).is_none());
            let mut bad = good.clone();
            bad.remove(index);
            assert!(summary_config(&record(&bad)).is_none());
            for suffix in [0, 0x80] {
                let mut bad = good.clone();
                bad[index].1.push(suffix);
                assert!(summary_config(&record(&bad)).is_none());
            }
            for byte in [0x80, 2, 9] {
                let mut bad = good.clone();
                if byte == 0x80 {
                    bad[index].1[0] |= byte;
                } else {
                    bad[index].1[1] = byte;
                }
                assert!(summary_config(&record(&bad)).is_none());
            }
        }
        for (offset, value) in [(0, 0), (21, 253), (23, 40)] {
            let mut bad = config.clone();
            bad[offset] = value;
            assert!(summary_config(&bad).is_none());
        }
        let mut bad = config;
        bad.push(0);
        assert!(summary_config(&bad).is_none());
    }

    #[test]
    fn profile_and_identifier_authority_cannot_disagree() {
        for offset in [1, 2, 12] {
            let mut bad = fixture_record();
            bad[offset] ^= 1;
            assert!(summary_config(&bad).is_none());
        }
        for (index, start) in [(0, 4), (1, 1)] {
            for offset in [0, 1, 11] {
                let mut bad = sets();
                let mut data = strict_rbsp(&bad[index].1, 2).unwrap();
                data[start + offset] ^= 1;
                bad[index].1 = escaped(bad[index].0, &data);
                assert!(summary_config(&record(&bad)).is_none());
            }
        }
        let mut bad = sets();
        bad[0].1[2] ^= 0x10;
        assert!(summary_config(&record(&bad)).is_none());
    }

    #[derive(Default)]
    struct Writer(Vec<bool>);
    impl Writer {
        fn u(&mut self, value: u32, width: u32) {
            self.0
                .extend((0..width).rev().map(|shift| value & (1 << shift) != 0));
        }
        fn ue(&mut self, value: u32) {
            let code = value + 1;
            let width = 32 - code.leading_zeros();
            self.0
                .extend(std::iter::repeat_n(false, (width - 1) as usize));
            self.u(code, width);
        }
        fn se(&mut self, value: i32) {
            self.ue(if value > 0 {
                2 * value as u32 - 1
            } else {
                2 * (-value) as u32
            });
        }
        fn finish(mut self, kind: u8) -> Vec<u8> {
            self.0.push(true);
            while !self.0.len().is_multiple_of(8) {
                self.0.push(false);
            }
            let data: Vec<u8> = self
                .0
                .chunks_exact(8)
                .map(|chunk| chunk.iter().fold(0, |n, bit| (n << 1) | u8::from(*bit)))
                .collect();
            escaped(kind, &data)
        }
    }

    fn picture(change: &str) -> Vec<u8> {
        let mut b = Writer::default();
        b.ue(if change == "id" { 64 } else { 0 });
        b.ue(u32::from(change == "sps"));
        b.u(0, 2);
        b.u(u32::from(change == "extra"), 3);
        b.u(2, 2);
        b.ue(if change == "refs" { 15 } else { 0 });
        b.ue(0);
        b.se(if change == "qp" { -27 } else { 0 });
        b.u(0, 2);
        b.u(1, 1);
        b.ue(if change == "delta" { 3 } else { 0 });
        b.se(if change == "chroma" { 13 } else { 0 });
        b.se(0);
        b.u(4, 4);
        b.u(u32::from(change == "tiles"), 1);
        b.u(0, 1);
        b.u(1, 1);
        b.u(u32::from(change == "beta"), 1);
        if change == "beta" {
            b.u(0, 2);
            b.se(7);
            b.se(0);
        }
        b.u(u32::from(change == "scaling"), 1);
        b.u(0, 1);
        b.ue(if change == "merge" { 4 } else { 0 });
        b.u(u32::from(change == "header"), 1);
        b.u(u32::from(change == "extension"), 1);
        b.finish(34)
    }

    #[test]
    fn complete_pps_ranges_and_trailing_bits_are_required() {
        assert_eq!(picture(""), sets()[2].1);
        for change in [
            "id",
            "sps",
            "extra",
            "refs",
            "qp",
            "delta",
            "chroma",
            "tiles",
            "beta",
            "scaling",
            "merge",
            "header",
            "extension",
        ] {
            let mut bad = sets();
            bad[2].1 = picture(change);
            assert!(summary_config(&record(&bad)).is_none(), "{change}");
        }
        let mut bad = sets();
        *bad[2].1.last_mut().unwrap() = 0;
        assert!(summary_config(&record(&bad)).is_none());
    }

    #[test]
    fn vps_structure_and_ordering_are_complete_and_consistent() {
        for (offset, mask) in [(0, 8), (0, 4), (1, 16), (1, 2), (1, 1), (2, 1)] {
            let mut bad = sets();
            let mut data = strict_rbsp(&bad[0].1, 2).unwrap();
            data[offset] ^= mask;
            bad[0].1 = escaped(32, &data);
            assert!(summary_config(&record(&bad)).is_none());
        }
        for change in ["buffer", "order", "layer", "sets", "timing", "extension"] {
            let mut b = Writer::default();
            b.u(0, 4);
            b.u(3, 2);
            b.u(0, 6);
            b.u(1, 4);
            b.u(65535, 16);
            for byte in hex("01600000009000000000001e") {
                b.u(u32::from(byte), 8);
            }
            b.u(1, 1);
            b.ue(if change == "buffer" { 16 } else { 4 });
            b.ue(if change == "order" { 5 } else { 2 });
            b.ue(5);
            b.u(u32::from(change == "layer"), 6);
            b.ue(u32::from(change == "sets"));
            b.u(u32::from(change == "timing"), 1);
            b.u(u32::from(change == "extension"), 1);
            let mut bad = sets();
            bad[0].1 = b.finish(32);
            assert!(summary_config(&record(&bad)).is_none(), "{change}");
        }
    }

    #[test]
    fn every_unit_type_and_slice_selector_is_checked() {
        let config = summary_config(&fixture_record()).unwrap();
        for kind in 0..64 {
            let irap = (16..=21).contains(&kind);
            let payload = if irap { 0xa0 } else { 0xc0 };
            let unit = [kind << 1, 1, payload];
            assert_eq!(
                slice_selects(&config, &unit),
                [0, 1, 6, 7, 8, 9, 16, 17, 18, 19, 20, 21]
                    .contains(&kind)
                    .then_some(irap),
                "type {kind}"
            );
            for byte in [0, 2, 9] {
                assert!(slice_selects(&config, &[kind << 1, byte, payload]).is_none());
            }
        }
        for unit in [&[40, 1, 0x90][..], &[2, 1, 0xa0], &[40, 1], &[40, 1, 0x20]] {
            assert!(slice_selects(&config, unit).is_none());
        }
    }

    #[test]
    fn sps_hrd_restrictions_extensions_and_unclosed_tails_refuse() {
        use super::super::{HevcVui, hevc_sps_end};
        assert!(hevc_sps_end(&mut Bits::new(&[0x08]), HevcVui::Present(true)).is_some());
        for byte in [0x48, 0x28, 0x18, 0x00] {
            assert!(hevc_sps_end(&mut Bits::new(&[byte]), HevcVui::Present(true)).is_none());
        }
    }

    #[test]
    fn temporal_switching_units_cannot_belong_to_a_single_layer_summary() {
        let config = summary_config(&fixture_record()).unwrap();
        for kind in 2..=5 {
            assert!(
                slice_selects(&config, &[kind << 1, 1, 0xc0]).is_none(),
                "type {kind}"
            );
        }
    }

    #[test]
    fn incomplete_sps_never_certifies_retained_picture_properties() {
        let nal = &sets()[1].1;
        let (info, facts) = hevc_sps_read(nal, false).unwrap();
        assert!(facts.complete);
        assert_eq!(info.pix_fmt.as_deref(), Some("yuv420p"));
        assert_eq!(info.coded_size, Some((64, 48)));
        assert_eq!(info.has_b_frames, Some(2));
        assert_eq!(info.frame_rate, Some((30000, 1001)));
        for end in 0..nal.len() {
            if let Some((partial, facts)) = hevc_sps_read(&nal[..end], false) {
                assert!(!facts.complete, "cut {end}");
                assert_eq!(partial.pix_fmt, info.pix_fmt);
                assert_eq!(partial.coded_size, info.coded_size);
                assert_eq!(partial.has_b_frames, info.has_b_frames);
            }
        }
    }
}
