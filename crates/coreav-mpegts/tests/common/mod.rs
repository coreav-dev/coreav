//! Hand-built transport streams for the tests: a writer of 188-byte
//! packets with continuity counters, the PAT and PMT sections, PES packets
//! and ADTS frames (ISO/IEC 13818-1, 13818-7).
#![allow(
    dead_code,
    clippy::disallowed_types,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

/// A writer of 188-byte transport packets with continuity counters.
#[derive(Default)]
pub struct Mux {
    pub out: Vec<u8>,
    pub counters: std::collections::HashMap<u16, u8>,
}

impl Mux {
    pub fn packet(&mut self, pid: u16, start: bool, payload: &[u8]) {
        let counter = self.counters.entry(pid).or_insert(0);
        let mut p = vec![
            0x47,
            (u8::from(start) << 6) | (pid >> 8) as u8,
            pid as u8,
            0x10 | *counter,
        ];
        *counter = (*counter + 1) & 0x0F;
        if payload.len() < 184 {
            // Pad with an adaptation field of stuffing.
            let pad = 184 - payload.len();
            p[3] |= 0x20;
            p.push((pad - 1) as u8);
            if pad > 1 {
                p.push(0);
                p.extend(std::iter::repeat_n(0xFF, pad - 2));
            }
        }
        p.extend_from_slice(&payload[..payload.len().min(184)]);
        assert_eq!(p.len(), 188);
        self.out.extend(p);
    }

    /// Splits `data` over packets of `pid`, the first a unit start.
    pub fn unit(&mut self, pid: u16, data: &[u8]) {
        for (i, chunk) in data.chunks(184).enumerate() {
            self.packet(pid, i == 0, chunk);
        }
    }

    pub fn section(&mut self, pid: u16, table_id: u8, id: u16, body: &[u8]) {
        let length = 5 + body.len() + 4;
        let mut s = vec![table_id, 0xB0 | (length >> 8) as u8, length as u8];
        s.extend(id.to_be_bytes());
        s.extend([0xC1, 0, 0]);
        s.extend(body);
        let crc = crc32(&s);
        s.extend(crc.to_be_bytes());
        let mut payload = vec![0u8];
        payload.extend(s);
        self.unit(pid, &payload);
    }

    pub fn pat(&mut self, programs: &[(u16, u16)]) {
        let body: Vec<u8> = programs
            .iter()
            .flat_map(|(n, pid)| {
                [
                    (n >> 8) as u8,
                    *n as u8,
                    0xE0 | (pid >> 8) as u8,
                    *pid as u8,
                ]
            })
            .collect();
        self.section(0, 0, 1, &body);
    }

    pub fn pmt(&mut self, pid: u16, program: u16, streams: &[(u8, u16)]) {
        self.pmt_with(pid, program, &[], streams);
    }

    /// A program map table whose program loop holds `descriptors`
    /// (13818-1 table 2-33), tag and length included.
    pub fn pmt_with(&mut self, pid: u16, program: u16, descriptors: &[u8], streams: &[(u8, u16)]) {
        let streams: Vec<_> = streams
            .iter()
            .map(|(kind, es)| (*kind, *es, Vec::new()))
            .collect();
        self.pmt_streams(pid, program, descriptors, &streams);
    }

    /// A program map table whose program loop and whose streams each hold
    /// descriptors, tag and length included.
    pub fn pmt_streams(
        &mut self,
        pid: u16,
        program: u16,
        descriptors: &[u8],
        streams: &[(u8, u16, Vec<u8>)],
    ) {
        let mut body = vec![0xE1, 0x00, 0xF0, descriptors.len() as u8];
        body.extend(descriptors);
        for (kind, es, stream) in streams {
            body.extend([
                *kind,
                0xE0 | (es >> 8) as u8,
                *es as u8,
                0xF0,
                stream.len() as u8,
            ]);
            body.extend(stream);
        }
        self.section(pid, 2, program, &body);
    }

    /// The registration descriptor (13818-1 2.6.8) of `identifier`.
    pub fn registration(identifier: &[u8; 4]) -> Vec<u8> {
        [&[0x05, 4][..], identifier].concat()
    }

    pub fn pes(&mut self, pid: u16, stream_id: u8, pts: u64, payload: &[u8], bounded: bool) {
        self.pes_times(pid, stream_id, pts, None, payload, bounded);
    }

    /// A PES packet that states a presentation time and, when `dts` gives
    /// one, a decoding time too (13818-1 2.4.3.7).
    pub fn pes_times(
        &mut self,
        pid: u16,
        stream_id: u8,
        pts: u64,
        dts: Option<u64>,
        payload: &[u8],
        bounded: bool,
    ) {
        let header = if dts.is_some() { 10 } else { 5 };
        let mut pes = vec![0, 0, 1, stream_id];
        let length = if bounded {
            payload.len() + 3 + header
        } else {
            0
        };
        pes.extend((length as u16).to_be_bytes());
        pes.extend([0x80, if dts.is_some() { 0xC0 } else { 0x80 }, header as u8]);
        pes.extend(timestamp(pts, if dts.is_some() { 3 } else { 2 }));
        if let Some(dts) = dts {
            pes.extend(timestamp(dts, 1));
        }
        pes.extend(payload);
        self.unit(pid, &pes);
    }
}

/// The five bytes of a timestamp under `marker`, the four bits before it
/// (ISO/IEC 13818-1 table 2-21).
pub fn timestamp(value: u64, marker: u8) -> [u8; 5] {
    [
        (marker << 4) | (((value >> 30) as u8 & 7) << 1) | 1,
        (value >> 22) as u8,
        (((value >> 15) as u8) << 1) | 1,
        (value >> 7) as u8,
        ((value as u8) << 1) | 1,
    ]
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// An ADTS frame of AAC LC, 48 kHz, stereo, `len` bytes.
pub fn adts(len: usize) -> Vec<u8> {
    let mut f = vec![
        0xFF,
        0xF1,
        0x4C,
        0x80 | ((len >> 11) as u8 & 3),
        (len >> 3) as u8,
        ((len as u8) << 5) | 0x1F,
        0xFC,
    ];
    f.resize(len, 0x21);
    f
}

/// A writer of the bits of a sequence parameter set.
#[derive(Default)]
pub struct Bits {
    out: Vec<u8>,
    bit: u8,
}

impl Bits {
    pub fn put(&mut self, value: u64, width: u8) {
        for i in (0..width).rev() {
            if self.bit == 0 {
                self.out.push(0);
            }
            let set = (value >> i) & 1 == 1;
            if set {
                let at = self.out.len() - 1;
                self.out[at] |= 1 << (7 - self.bit);
            }
            self.bit = (self.bit + 1) & 7;
        }
    }

    /// `ue(v)`, the Exp-Golomb coding of ITU-T H.264 9.1.
    pub fn ue(&mut self, value: u32) {
        let shifted = u64::from(value) + 1;
        let width = 64 - shifted.leading_zeros() as u8;
        self.put(0, width - 1);
        self.put(shifted, width);
    }

    /// The bytes, with the trailing bits and the emulation prevention of
    /// ITU-T H.264 7.4.1.1.
    pub fn nal(mut self, header: u8) -> Vec<u8> {
        self.put(1, 1);
        while self.bit != 0 {
            self.put(0, 1);
        }
        let mut out = vec![header];
        let mut zeros = 0;
        for byte in self.out {
            if zeros == 2 && byte <= 3 {
                out.push(3);
                zeros = 0;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            out.push(byte);
        }
        out
    }
}

/// A sequence parameter set of baseline profile for a picture of
/// `width` by `height` whole macroblocks, with the timing information of
/// ITU-T H.264 E.2.1 when `timing` gives one (`num_units_in_tick`,
/// `time_scale`).
pub fn h264_sps(width: u32, height: u32, timing: Option<(u32, u32)>) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(66, 8); // profile_idc: baseline, so no chroma format block
    bits.put(0xC0, 8); // constraint_set0_flag and constraint_set1_flag
    bits.put(30, 8); // level_idc
    bits.ue(0); // seq_parameter_set_id
    bits.ue(0); // log2_max_frame_num_minus4
    bits.ue(0); // pic_order_cnt_type
    bits.ue(0); // log2_max_pic_order_cnt_lsb_minus4
    bits.ue(1); // max_num_ref_frames
    bits.put(0, 1); // gaps_in_frame_num_value_allowed_flag
    bits.ue(width / 16 - 1);
    bits.ue(height / 16 - 1);
    bits.put(1, 1); // frame_mbs_only_flag
    bits.put(1, 1); // direct_8x8_inference_flag
    bits.put(0, 1); // frame_cropping_flag
    bits.put(u64::from(timing.is_some()), 1); // vui_parameters_present_flag
    if let Some((units, scale)) = timing {
        bits.put(0, 1); // aspect_ratio_info_present_flag
        bits.put(0, 1); // overscan_info_present_flag
        bits.put(0, 1); // video_signal_type_present_flag
        bits.put(0, 1); // chroma_loc_info_present_flag
        bits.put(1, 1); // timing_info_present_flag
        bits.put(u64::from(units), 32);
        bits.put(u64::from(scale), 32);
        bits.put(1, 1); // fixed_frame_rate_flag
        bits.put(0, 1); // nal_hrd_parameters_present_flag
        bits.put(0, 1); // vcl_hrd_parameters_present_flag
        bits.put(0, 1); // pic_struct_present_flag
        bits.put(0, 1); // bitstream_restriction_flag
    }
    bits.nal(0x67)
}

/// A picture parameter set with the two identifiers and the flags a
/// baseline stream needs (ITU-T H.264 7.3.2.2).
pub fn h264_pps() -> Vec<u8> {
    let mut bits = Bits::default();
    bits.ue(0); // pic_parameter_set_id
    bits.ue(0); // seq_parameter_set_id
    bits.put(0, 1); // entropy_coding_mode_flag
    bits.put(0, 1); // bottom_field_pic_order_in_frame_present_flag
    bits.ue(0); // num_slice_groups_minus1
    bits.ue(0); // num_ref_idx_l0_default_active_minus1
    bits.ue(0); // num_ref_idx_l1_default_active_minus1
    bits.put(0, 1); // weighted_pred_flag
    bits.put(0, 2); // weighted_bipred_idc
    bits.ue(0); // pic_init_qp_minus26 (se)
    bits.ue(0); // pic_init_qs_minus26 (se)
    bits.ue(0); // chroma_qp_index_offset (se)
    bits.put(1, 1); // deblocking_filter_control_present_flag
    bits.put(0, 1); // constrained_intra_pred_flag
    bits.put(0, 1); // redundant_pic_cnt_present_flag
    bits.nal(0x68)
}
