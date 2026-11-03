set -euo pipefail
umask 077
mkdir -p out
chmod +x bin/*
sudo apt-get update -q > out/apt.log 2>&1
sudo apt-get install -y -q --no-install-recommends linux-perf binutils g++ >> out/apt.log 2>&1
sudo sysctl -w kernel.perf_event_paranoid=-1 > out/perf-policy.txt
candidate=$(python3 -I -c 'import json; print(json.load(open("builds.json"))["candidate"])')
python3 -I scripts/vp9_pixel_round.py capture --root "$PWD" --candidate "$candidate" \
  --out "$PWD/out/capture" --max-seconds 6000 > out/capture.log 2>&1
