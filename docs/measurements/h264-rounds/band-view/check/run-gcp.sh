#!/bin/bash
# run-gcp.sh [ZONE]: the check's run. Stages the upload directory (job.sh, ../../hints-8t/rec_hints.py, es/ = the 32
# training streams, prof-<arm> from build.sh, ffmpeg = the BtbN build) and runs job.sh on one SPOT c4-standard-16 with
# one thread a core, PMU STANDARD, made and deleted by scripts/gcp_vm_job.py, capped at 1.2 USD (1 hour at 1.20 USD).
set -euo pipefail
Z=${1:-us-central1-a}; H=$(cd "$(dirname "$0")" && pwd); U=~/r5/up-check; rm -rf $U; mkdir -p $U/es
cp $H/job.sh $H/../../hints-8t/rec_hints.py $U/; cp ~/tr/es/*.h264 $U/es/; cp ~/r5/prof-base ~/r5/prof-k $U/
cp ~/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg $U/ffmpeg
python3 -I $(git -C $H rev-parse --show-toplevel)/scripts/gcp_vm_job.py --upload $U --job job.sh --download ~/r5/res-check --zone $Z \
  --machine c4-standard-16 --threads-per-core 1 --provisioning spot --usd-per-hour 1.20 --max-hours 1 --cap-usd 1.2
