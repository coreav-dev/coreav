#!/bin/bash
# gcp8.sh NAME BASEBIN KBIN ZONE [extra args]: the eight-thread arms of a round on Compute Engine with prebuilt arms (base, k; no build here, so a one-thread run on this VM is not disturbed).
# Tools of origin/main (h264_parity_gcp.py, gcp_vm_job.py, h264_parity.py); output ~/ab/NAME/gcp-outN.
set -euo pipefail
NAME=$1; BB=$2; KB=$3; ZONE=$4; shift 4
R=$HOME/ab/$NAME; W=$R/gt; mkdir -p $W/tools
REPO=$HOME/coreav; git -C $REPO fetch -q origin
for f in h264_parity_gcp.py gcp_vm_job.py h264_parity.py; do git -C $REPO show origin/main:scripts/$f > $W/tools/$f; done
echo "tools of origin/main $(git -C $REPO rev-parse origin/main)" > $R/tools.txt
n=1; while [ -e $R/gcp-out$n ]; do n=$((n+1)); done
python3 $W/tools/h264_parity_gcp.py --out $R/gcp-out$n --es-dir ${ESDIR:-$HOME/tr/es} --zone $ZONE --max-hours 4.2 --cap-usd 6 \
  --decoder base=$BB --decoder k=$KB \
  --ffmpeg $HOME/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg \
  --gate-stream $HOME/tr/es/36501d448eaa.0.720p.h264 --null-arm base "$@"
