#!/usr/bin/env bash
# Generates the clips the release smoke test transcodes
# (test/release-smoke.sh; docs/design/release.md, "Dry run"): one second of
# FFmpeg's test pattern, 10 frames at 320x240, as AV1 in MP4, VP9 in WebM
# and H.264 in MP4, and a second of a sine tone as AAC LC in M4A:
#   test/release-clips.sh DIR
# Needs an ffmpeg program with libx264, libvpx and an AV1 encoder (the
# pinned FFmpeg, scripts/install-oracle-ffmpeg.sh).
set -euo pipefail
dir="$1"
mkdir -p "$dir"
encoders="$(ffmpeg -hide_banner -encoders)"
av1=
for encoder in libsvtav1 libaom-av1 librav1e; do
  if grep -qw -- "$encoder" <<< "$encoders"; then av1="$encoder"; break; fi
done
[ -n "$av1" ] || { echo "error: ffmpeg has no AV1 encoder" >&2; exit 1; }
video=(-f lavfi -i testsrc2=size=320x240:rate=10:duration=1 -pix_fmt yuv420p)
ff=(ffmpeg -nostdin -hide_banner -loglevel error -y)
"${ff[@]}" "${video[@]}" -c:v "$av1" "$dir/av1.mp4"
"${ff[@]}" "${video[@]}" -c:v libvpx-vp9 -deadline realtime -cpu-used 8 "$dir/vp9.webm"
"${ff[@]}" "${video[@]}" -c:v libx264 -preset ultrafast "$dir/h264.mp4"
"${ff[@]}" -f lavfi -i sine=frequency=440:sample_rate=48000:duration=1 -c:a aac "$dir/aac.m4a"
ls -l "$dir"
