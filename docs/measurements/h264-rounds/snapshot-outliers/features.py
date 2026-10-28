"""FFMPEG=path python3 -I features.py STREAM... -> per stream: parameter-set and slice-header fields from FFmpeg trace_headers (a black box; the 300 first pictures)."""
import re, subprocess, sys, os, json, collections
FF = os.environ['FFMPEG']
FP = os.path.dirname(FF) + '/ffprobe'
SPS = ['profile_idc','constraint_set0_flag','constraint_set1_flag','constraint_set3_flag','level_idc','chroma_format_idc','bit_depth_luma_minus8','bit_depth_chroma_minus8','pic_order_cnt_type','max_num_ref_frames','pic_width_in_mbs_minus1','pic_height_in_map_units_minus1','frame_mbs_only_flag','mb_adaptive_frame_field_flag','direct_8x8_inference_flag','frame_cropping_flag','frame_crop_left_offset','frame_crop_right_offset','frame_crop_top_offset','frame_crop_bottom_offset','seq_scaling_matrix_present_flag','qpprime_y_zero_transform_bypass_flag','separate_colour_plane_flag']
PPS = ['entropy_coding_mode_flag','num_slice_groups_minus1','num_ref_idx_l0_default_active_minus1','num_ref_idx_l1_default_active_minus1','weighted_pred_flag','weighted_bipred_idc','pic_init_qp_minus26','chroma_qp_index_offset','second_chroma_qp_index_offset','deblocking_filter_control_present_flag','constrained_intra_pred_flag','redundant_pic_cnt_present_flag','transform_8x8_mode_flag','pic_scaling_matrix_present_flag']
SH = ['slice_type','field_pic_flag','bottom_field_flag','direct_spatial_mv_pred_flag','num_ref_idx_active_override_flag','disable_deblocking_filter_idc','cabac_init_idc','nal_unit_type','nal_ref_idc','redundant_pic_cnt','long_term_reference_flag','num_ref_idx_l0_active_minus1','num_ref_idx_l1_active_minus1','luma_log2_weight_denom','ref_pic_list_modification_flag_l0','ref_pic_list_modification_flag_l1','adaptive_ref_pic_marking_mode_flag','first_mb_in_slice']
line = re.compile(r'\]\s+\d+\s+(\S+)\s+[01]* = (-?\d+)$')
def run(path, frames):
    out = subprocess.run([FF,'-nostdin','-v','debug','-i',path,'-frames:v',str(frames),'-c','copy','-bsf:v','trace_headers','-f','null','-'],capture_output=True,text=True).stderr
    vals = {'sps': collections.defaultdict(collections.Counter), 'pps': collections.defaultdict(collections.Counter), 'sh': collections.defaultdict(collections.Counter)}
    cur = None; slices = 0; pics = collections.Counter(); first_mb0 = 0; nal = collections.Counter()
    for l in out.splitlines():
        if 'Sequence Parameter Set' in l: cur = 'sps'; continue
        if 'Picture Parameter Set' in l: cur = 'pps'; continue
        if 'Slice Header' in l: cur = 'sh'; slices += 1; continue
        if 'Access Unit Delimiter' in l or 'Supplemental' in l or 'Slice Data' in l: cur = None; continue
        m = line.search(l)
        if m and cur:
            n, v = m.group(1), int(m.group(2))
            if n == 'first_mb_in_slice' and v == 0: first_mb0 += 1
            if (cur=='sps' and n in SPS) or (cur=='pps' and n in PPS) or (cur=='sh' and n in SH):
                vals[cur][n][v] += 1
    probe = subprocess.run([FP,'-v','error','-show_entries','stream=profile,level,pix_fmt,width,height,coded_width,coded_height,field_order,has_b_frames,refs,color_range,color_space,sample_aspect_ratio','-of','json',path],capture_output=True,text=True).stdout
    p = json.loads(probe)['streams'][0]
    return vals, slices, first_mb0, p
if __name__ == '__main__':
    for path in sys.argv[1:]:
        vals, slices, pics, p = run(path, 300)
        print('==', os.path.basename(path).split('.')[0], 'slices', slices, 'pics', pics, 'slices/pic %.2f' % (slices / max(pics,1)))
        print(' probe', p)
        for k in ('sps','pps','sh'):
            for n, c in vals[k].items():
                print(' ', k, n, dict(c))
