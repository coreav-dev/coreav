#!/usr/bin/env bash
# Generate MPEG transport stream fixtures (docs/design/mpegts.md) with the
# ffmpeg binary, used as a black box only, and rewrite some of them with
# scripts/ts_craft.py into structures ffmpeg never writes.
# Usage: scripts/gen-fixtures-ts.sh /absolute/output/dir
#
# Writes $OUT/MANIFEST with "generated NAME" or "skipped NAME REASON" lines,
# like scripts/gen-fixtures.sh. Required fixtures fail by default; set
# COREAV_REQUIRE_ALL_FIXTURES=0 only for best-effort local generation.
set -euo pipefail

OUT="${1:?usage: gen-fixtures-ts.sh /absolute/output/dir}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$OUT"
MANIFEST="$OUT/MANIFEST"
: > "$MANIFEST"
REQUIRE_ALL="${COREAV_REQUIRE_ALL_FIXTURES:-1}"
CRAFT=(python3 -I "$ROOT/scripts/ts_craft.py")

FF=(ffmpeg -hide_banner -loglevel error -y)
V=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=3")
A=(-f lavfi -i "sine=frequency=440:duration=3:sample_rate=48000")
A44=(-f lavfi -i "sine=frequency=440:duration=3:sample_rate=44100")
H264=(-c:v libx264 -pix_fmt yuv420p -g 25 -bf 3)
TS=(-f mpegts)

generated() { echo "generated $1" >> "$MANIFEST"; }

skip() { # NAME REASON [best-effort]
  echo "skipped $1 $2" >> "$MANIFEST"
  echo "skipping $1: $2" >&2
  if [ "$REQUIRE_ALL" = "1" ] && [ "${3:-}" != "best-effort" ]; then
    echo "COREAV_REQUIRE_ALL_FIXTURES=1: refusing to skip $1" >&2
    exit 1
  fi
}

# try_gen NAME FFMPEG_ARGS...   (required)
try_gen() {
  local name="$1"; shift
  if "${FF[@]}" "$@" "$OUT/$name" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "ffmpeg-failed(see-$name.log)"
  fi
}

# craft NAME SOURCE OP [ARGS...]   (required): scripts/ts_craft.py
craft() {
  local name="$1" source="$2"; shift 2
  if [ ! -f "$OUT/$source" ]; then
    skip "$name" "missing-source-$source"
  elif "${CRAFT[@]}" "$@" "$OUT/$source" "$OUT/$name" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "craft-failed(see-$name.log)"
  fi
}

# --- Codecs ---
try_gen ts_h264_aac.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac "${TS[@]}"
try_gen ts_h264_nobframes.ts "${V[@]}" "${A[@]}" -c:v libx264 -g 50 -bf 0 -c:a aac "${TS[@]}"
try_gen ts_h264_opengop.ts "${V[@]}" -c:v libx264 -x264-params keyint=30:min-keyint=30:open-gop=1:bframes=2 "${TS[@]}"
try_gen ts_h264_intrarefresh.ts "${V[@]}" -c:v libx264 -x264-params keyint=30:intra-refresh=1 "${TS[@]}"
try_gen ts_h264_high10.ts -f lavfi -i "testsrc2=size=854x480:rate=30000/1001:duration=3" -c:v libx264 -pix_fmt yuv420p10le "${TS[@]}"
try_gen ts_h264_1080.ts -f lavfi -i "testsrc2=size=1920x1080:rate=25:duration=1" -c:v libx264 -preset ultrafast -color_primaries bt709 -color_trc bt709 -colorspace bt709 "${TS[@]}"
try_gen ts_hevc_aac.ts "${V[@]}" "${A[@]}" -c:v libx265 -x265-params log-level=none:bframes=3 -c:a aac "${TS[@]}"
try_gen ts_hevc_main10.ts "${V[@]}" -c:v libx265 -x265-params log-level=none -pix_fmt yuv420p10le "${TS[@]}"
try_gen ts_mpeg2_mp2.ts "${V[@]}" "${A44[@]}" -c:v mpeg2video -bf 2 -c:a mp2 "${TS[@]}"
try_gen ts_mpeg1_mp3.ts "${V[@]}" "${A44[@]}" -c:v mpeg1video -c:a libmp3lame "${TS[@]}"
try_gen ts_h264_ac3.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a ac3 "${TS[@]}"
try_gen ts_h264_eac3.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a eac3 "${TS[@]}"
try_gen ts_h264_ac3_51.ts "${V[@]}" -f lavfi -i "sine=duration=3:sample_rate=48000" -filter_complex "[1:a]pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0[a]" -map 0:v -map "[a]" "${H264[@]}" -c:a ac3 "${TS[@]}"
try_gen ts_audio_only.ts "${A[@]}" -c:a aac "${TS[@]}"
try_gen ts_video_only.ts "${V[@]}" "${H264[@]}" "${TS[@]}"
try_gen ts_aac_latm.ts "${A[@]}" -c:a aac -mpegts_flags latm "${TS[@]}"
try_gen ts_opus.ts "${A[@]}" -c:a libopus "${TS[@]}"
# --- Programs, streams, packetization ---
try_gen ts_two_programs.ts "${V[@]}" "${A[@]}" -map 0:v -map 1:a -map 0:v -map 1:a "${H264[@]}" -c:a aac \
  -program title=A:st=0:st=1 -program title=B:st=2:st=3 "${TS[@]}"
try_gen ts_languages.ts "${V[@]}" "${A[@]}" -map 0:v -map 1:a -map 1:a "${H264[@]}" -c:a aac \
  -metadata:s:a:0 language=eng -metadata:s:a:1 language=deu "${TS[@]}"
try_gen ts_many_audio_per_pes.ts "${A[@]}" -c:a aac -pes_payload_size 4000 "${TS[@]}"
try_gen ts_one_frame_per_pes.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -pes_payload_size 0 "${TS[@]}"
try_gen ts_resend_headers.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -mpegts_flags resend_headers "${TS[@]}"
try_gen ts_pcr_period.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -pcr_period 100 -muxrate 2000000 "${TS[@]}"
try_gen ts_long.ts -f lavfi -i "testsrc2=size=64x48:rate=10:duration=40" -f lavfi -i "sine=duration=40:sample_rate=48000" \
  -c:v libx264 -g 10 -c:a aac "${TS[@]}"
try_gen ts_m2ts.m2ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -mpegts_m2ts_mode 1 -f mpegts
try_gen ts_h264_ac3.m2ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a ac3 -mpegts_m2ts_mode 1 -f mpegts
# --- Timestamps near the 33-bit wrap (95443.7 s) ---
try_gen ts_wrap_near.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -output_ts_offset 95442 "${TS[@]}"
try_gen ts_wrap_far.ts -f lavfi -i "testsrc2=size=64x48:rate=5:duration=80" -c:v libx264 -g 5 -output_ts_offset 95377 "${TS[@]}"
try_gen ts_start_late.ts "${V[@]}" "${A[@]}" "${H264[@]}" -c:a aac -output_ts_offset 30000 "${TS[@]}"
# --- Crafted structures ---
craft ts_prefix_junk.ts ts_h264_aac.ts prefix 77
craft ts_lost_packets.ts ts_h264_aac.ts lose 0x100 37
craft ts_lost_sync.ts ts_h264_aac.ts junk 40 113
craft ts_204.ts ts_h264_aac.ts pad204
craft ts_duplicates.ts ts_h264_aac.ts duplicate 0x101
craft ts_transport_errors.ts ts_h264_aac.ts error 0x101
craft ts_nulls.ts ts_h264_aac.ts nulls 5
craft ts_pmt_change.ts ts_h264_aac.ts pmtchange
craft ts_truncated.ts ts_h264_aac.ts cut 0.63
craft ts_truncated_m2ts.m2ts ts_m2ts.m2ts cut 0.5
# The decoding time a PES packet states when it does not differ from the
# presentation time, and when it is left out (KD-TS-DECODE-TIME).
craft ts_dts_equals_pts.ts ts_h264_aac.ts dtspts 0x100
craft ts_pts_only.ts ts_h264_aac.ts ptsonly 0x100
# A PID no table declares, beside two programs; and the same file with the
# second program's table held back, so that the reference lists that PID
# before the second program's streams and this reader after them
# (KD-TS-STREAM-ORDER).
craft ts_undeclared_pid.ts ts_two_programs.ts inject 0x1FF 20
craft ts_undeclared_first.ts ts_undeclared_pid.ts delay 0x1001 0.05
# One video PES packet, which only the end of the file completes: it
# carries the stream's parameter sets.
craft ts_one_pes.ts ts_h264_aac.ts onepes 0x100
# Sound whose PES packets state a decoding time of their own, 600 ticks
# before the presentation time (KD-TS-FRAME-TIME).
craft ts_audio_dts.ts ts_h264_aac.ts audiodts 0x101 600

echo "fixtures written to $OUT (see $OUT/MANIFEST)"
