//! Streams built bit by bit, for the tests of this crate: the integration
//! tests include this file, and so does the library's own test build
//! (`src/lib.rs`), so that a unit test can decode the same streams and
//! look at state the public interface does not show.
#![forbid(unsafe_code)]
// Each user takes only some of these.
#![allow(dead_code)]

/// A most-significant-bit-first bit writer, enough for the syntax these
/// tests build.
#[derive(Default)]
pub struct Writer {
    bits: Vec<u8>,
}

impl Writer {
    pub fn u(&mut self, n: u32, v: u32) -> &mut Self {
        for i in (0..n).rev() {
            self.bits.push(((v >> i) & 1) as u8);
        }
        self
    }

    pub fn flag(&mut self, v: bool) -> &mut Self {
        self.u(1, u32::from(v))
    }

    /// ue(v) (clause 9.1).
    pub fn ue(&mut self, v: u32) -> &mut Self {
        let value = v + 1;
        let len = 32 - value.leading_zeros();
        self.u(len - 1, 0).u(len, value)
    }

    /// se(v) (clause 9.1.1).
    pub fn se(&mut self, v: i32) -> &mut Self {
        let code = if v > 0 {
            2 * v as u32 - 1
        } else {
            2 * (-v) as u32
        };
        self.ue(code)
    }

    pub fn align_zero(&mut self) -> &mut Self {
        while self.bits.len() % 8 != 0 {
            self.bits.push(0);
        }
        self
    }

    pub fn bytes(&mut self, data: &[u8]) -> &mut Self {
        for &b in data {
            self.u(8, u32::from(b));
        }
        self
    }

    /// The RBSP with its trailing bits, as a NAL unit payload.
    pub fn finish(&mut self) -> Vec<u8> {
        self.bits.push(1);
        while self.bits.len() % 8 != 0 {
            self.bits.push(0);
        }
        self.bits
            .chunks(8)
            .map(|c| c.iter().fold(0u8, |v, &b| (v << 1) | b))
            .collect()
    }
}

/// A NAL unit with its start code and header.
pub fn nal(ref_idc: u8, kind: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, (ref_idc << 5) | kind];
    out.extend(payload);
    out
}

/// A non-IDR I slice whose `dec_ref_pic_marking` is written only when the
/// NAL is a reference one (clause 7.3.3).
pub fn non_idr_pcm_slice_ref(
    frame_num: u32,
    poc_lsb: u32,
    macroblocks: usize,
    reference: bool,
) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb);
    if reference {
        w.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    w.se(0); // slice_qp_delta
    for addr in 0..macroblocks {
        w.ue(25);
        w.align_zero();
        w.bytes(&pcm_samples(addr));
    }
    w.finish()
}

/// A P slice whose every macroblock is skipped, with
/// `dec_ref_pic_marking` written only for a reference NAL (clause 7.3.3).
pub fn skipped_p_slice_ref(
    frame_num: u32,
    poc_lsb: u32,
    macroblocks: usize,
    reference: bool,
) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(5) // slice_type: P, and every slice of the picture is P
        .ue(0) // pic_parameter_set_id
        .u(4, frame_num)
        .u(6, poc_lsb)
        .flag(false) // num_ref_idx_active_override_flag
        .flag(false); // ref_pic_list_modification_flag_l0
    if reference {
        w.flag(false); // adaptive_ref_pic_marking_mode_flag
    }
    w.se(0) // slice_qp_delta
        .ue(macroblocks as u32); // mb_skip_run
    w.finish()
}

/// A sequence parameter set that allows frame_num gaps (clause 8.2.5.2).
pub fn sps_with_gaps(mb_w: u32, mb_h: u32) -> Vec<u8> {
    let mut w = Writer::default();
    w.u(8, 66).u(8, 0xc0).u(8, 10);
    w.ue(0) // seq_parameter_set_id
        .ue(0) // log2_max_frame_num_minus4
        .ue(0) // pic_order_cnt_type
        .ue(2) // log2_max_pic_order_cnt_lsb_minus4
        .ue(1) // max_num_ref_frames
        .flag(true) // gaps_in_frame_num_value_allowed_flag
        .ue(mb_w - 1)
        .ue(mb_h - 1)
        .flag(true) // frame_mbs_only_flag
        .flag(true) // direct_8x8_inference_flag
        .flag(false) // frame_cropping_flag
        .flag(false); // vui_parameters_present_flag
    w.finish()
}

/// A picture parameter set with CAVLC and no deblocking control.
pub fn pps() -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // pic_parameter_set_id
        .ue(0) // seq_parameter_set_id
        .flag(false) // entropy_coding_mode_flag
        .flag(false) // bottom_field_pic_order_in_frame_present_flag
        .ue(0) // num_slice_groups_minus1
        .ue(0) // num_ref_idx_l0_default_active_minus1
        .ue(0) // num_ref_idx_l1_default_active_minus1
        .flag(false) // weighted_pred_flag
        .u(2, 0) // weighted_bipred_idc
        .se(0) // pic_init_qp_minus26
        .se(0) // pic_init_qs_minus26
        .se(0) // chroma_qp_index_offset
        .flag(false) // deblocking_filter_control_present_flag
        .flag(false) // constrained_intra_pred_flag
        .flag(false); // redundant_pic_cnt_present_flag
    w.finish()
}

/// An IDR slice whose macroblocks are all I_PCM, with `samples(addr)`
/// giving each one's 384 samples.
pub fn idr_pcm_slice(macroblocks: usize, samples: impl Fn(usize) -> [u8; 384]) -> Vec<u8> {
    let mut w = Writer::default();
    w.ue(0) // first_mb_in_slice
        .ue(7) // slice_type: I, and every slice of the picture is I
        .ue(0) // pic_parameter_set_id
        .u(4, 0) // frame_num
        .ue(0) // idr_pic_id
        .u(6, 0) // pic_order_cnt_lsb
        .flag(false) // no_output_of_prior_pics_flag
        .flag(false) // long_term_reference_flag
        .se(0); // slice_qp_delta
    for addr in 0..macroblocks {
        w.ue(25); // mb_type: I_PCM
        w.align_zero();
        w.bytes(&samples(addr));
    }
    w.finish()
}

/// The samples of macroblock `addr`: luma rises with the sample index and
/// chroma is fixed per macroblock. No sample is 0, so the stream needs no
/// emulation prevention bytes.
pub fn pcm_samples(addr: usize) -> [u8; 384] {
    let mut out = [0u8; 384];
    for (i, s) in out.iter_mut().enumerate() {
        *s = match i {
            0..=255 => 1 + ((addr * 16 + i) % 200) as u8,
            256..=319 => 40 + addr as u8,
            _ => 200 - addr as u8,
        };
    }
    out
}
