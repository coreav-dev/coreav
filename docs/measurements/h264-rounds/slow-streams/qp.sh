#!/bin/bash
# qp.sh FFMPEG ES_DIR: per training stream, the slice QPs (26 + pic_init_qp_minus26 + slice_qp_delta) and the
# FilterOffsetA (2 x slice_alpha_c0_offset_div2, 0 where absent) of its slices over the class frame count, read from
# FFmpeg's trace_headers bitstream filter (a black box); the share of slices whose QP + FilterOffsetA is below 16.
for f in $(ls $2); do c=$(echo $f | cut -d. -f3); case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  $1 -hide_banner -loglevel trace -i $2/$f -c copy -bsf:v trace_headers -frames:v $n -f null - 2>&1 | \
  grep -E "pic_init_qp_minus26|slice_qp_delta|slice_alpha_c0_offset_div2|first_mb_in_slice" | \
  awk -v s=${f:0:12} '/pic_init_qp/{p=$NF} /first_mb_in_slice/{fin(); a=0} /slice_qp_delta/{q=26+p+$NF; have=1} /alpha_c0/{a=2*$NF}
    function fin(){ if(!have) return; n++; t+=q; if(min==""||q<min)min=q; if(q>max)max=q; if(q+a<16)lo++; ta+=a; have=0 }
    END{fin(); printf "%s slices %5d QP mean %4.1f min %2d max %2d FilterOffsetA mean %+5.1f QP+offset<16 %.2f\n", s, n, t/n, min, max, ta/n, lo/n}'
done
