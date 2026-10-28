"""FFMPEG=path python3 -I table.py ES_DIR -> features.tsv: one row per training stream, from features.py."""
import sys, os, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import features as f
ES = sys.argv[1]
def one(c, n, d=None):
    return ','.join(str(k) for k in sorted(c[n])) if n in c else (d or '-')
rows = []
for name in sorted(os.listdir(ES)):
    if not name.endswith('.h264'): continue
    v, sl, pics, p = f.run(ES + '/' + name, 300)
    s, pp, h = v['sps'], v['pps'], v['sh']
    st = collections.Counter()
    for k, c in h['slice_type'].items(): st[k % 5] += c
    rows.append((name.split('.')[0], name.split('.')[2], p.get('profile'), one(s,'level_idc'), one(pp,'entropy_coding_mode_flag'), one(pp,'transform_8x8_mode_flag','0'), one(pp,'weighted_pred_flag'), one(pp,'weighted_bipred_idc'), one(s,'pic_order_cnt_type'), one(s,'max_num_ref_frames'), one(s,'frame_mbs_only_flag'), one(s,'mb_adaptive_frame_field_flag','-'), one(s,'frame_cropping_flag'), p.get('pix_fmt'), '%.1f' % (sl/max(pics,1)), 'P%d B%d I%d' % (st[0], st[1], st[2]+st[4]), one(h,'field_pic_flag','-'), one(pp,'num_slice_groups_minus1'), one(h,'disable_deblocking_filter_idc'), one(pp,'constrained_intra_pred_flag'), one(s,'chroma_format_idc','-')))
hdr = ('id','cls','profile','lvl','cabac','transform8x8','weighted_pred','weighted_bipred_idc','poc_type','refs','frame_mbs_only','mbaff','crop','pix','slices_per_pic','slices_by_type','field','slice_groups_minus1','dbf_idc','constrained_intra','chroma')
print('\t'.join(hdr))
for r in rows: print('\t'.join(map(str, r)))
