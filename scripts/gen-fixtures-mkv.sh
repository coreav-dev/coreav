#!/usr/bin/env bash
# Generate Matroska and WebM fixtures (docs/design/mkv.md) with the ffmpeg
# binary, used as a black box only, and rewrite some of them with
# scripts/mkv_craft.py into structures ffmpeg never writes.
# Usage: scripts/gen-fixtures-mkv.sh /absolute/output/dir
#
# Writes $OUT/MANIFEST with "generated NAME" or "skipped NAME REASON" lines,
# like scripts/gen-fixtures.sh. Required fixtures fail by default; set
# COREAV_REQUIRE_ALL_FIXTURES=0 only for best-effort local generation.
set -euo pipefail

OUT="${1:?usage: gen-fixtures-mkv.sh /absolute/output/dir}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$OUT"
MANIFEST="$OUT/MANIFEST"
: > "$MANIFEST"
REQUIRE_ALL="${COREAV_REQUIRE_ALL_FIXTURES:-1}"
CRAFT=(python3 -I "$ROOT/scripts/mkv_craft.py")

FF=(ffmpeg -hide_banner -loglevel error -y)
VSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=2")
ASRC=(-f lavfi -i "sine=frequency=440:duration=2:sample_rate=48000")
H264=(-c:v libx264 -pix_fmt yuv420p -g 25 -bf 3)
VP9=(-c:v libvpx-vp9 -deadline realtime -cpu-used 8 -g 25)

ENCODERS="$(ffmpeg -hide_banner -encoders 2>/dev/null || true)"

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

# craft NAME SOURCE OP [ARGS...]   (required): scripts/mkv_craft.py
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

# Crafting sources are made first and are not fixtures themselves.
SRC="$OUT/.sources"
mkdir -p "$SRC"

# --- WebM: the browser codecs ---
try_gen webm_vp9_opus.webm "${VSRC[@]}" "${ASRC[@]}" "${VP9[@]}" -c:a libopus -shortest
try_gen webm_vp8_vorbis.webm "${VSRC[@]}" "${ASRC[@]}" -c:v libvpx -deadline realtime \
  -cpu-used 8 -g 25 -c:a libvorbis -shortest
AV1=""
for e in libsvtav1 libaom-av1 librav1e; do
  if grep -qw -- "$e" <<< "$ENCODERS"; then AV1="$e"; break; fi
done
if [ -n "$AV1" ]; then
  extra=()
  [ "$AV1" = libaom-av1 ] && extra=(-cpu-used 8)
  try_gen webm_av1_opus.webm "${VSRC[@]}" "${ASRC[@]}" -c:v "$AV1" "${extra[@]}" -g 25 \
    -c:a libopus -shortest
else
  skip webm_av1_opus.webm "no-AV1-encoder"
fi
# A live recording: written to a pipe, so the Segment and Clusters have
# unknown sizes and there is no Duration and no Cues (RFC 9559 23.2).
if "${FF[@]}" "${VSRC[@]}" "${ASRC[@]}" "${VP9[@]}" -c:a libopus -shortest -f webm pipe:1 \
    > "$OUT/webm_live.webm" 2> "$OUT/webm_live.webm.log"; then
  rm -f "$OUT/webm_live.webm.log"; generated webm_live.webm
else
  rm -f "$OUT/webm_live.webm"; skip webm_live.webm "ffmpeg-failed"
fi
try_gen webm_audio_opus.webm "${ASRC[@]}" -c:a libopus
try_gen webm_cues_front.webm "${VSRC[@]}" "${VP9[@]}" -cues_to_front 1
try_gen webm_many_clusters.webm "${VSRC[@]}" "${ASRC[@]}" "${VP9[@]}" -c:a libopus -shortest \
  -cluster_time_limit 100

# --- Matroska ---
try_gen mkv_h264_aac.mkv "${VSRC[@]}" "${ASRC[@]}" "${H264[@]}" -c:a aac -shortest
try_gen mkv_hevc.mkv "${VSRC[@]}" -c:v libx265 -x265-params log-level=error
try_gen mkv_rotate_ccw90.mkv -display_rotation:v:0 90 -i "$OUT/mkv_h264_aac.mkv" -map 0 -c copy
try_gen mkv_rotate_ccw270.mkv -display_rotation:v:0 270 -i "$OUT/mkv_h264_aac.mkv" -map 0 -c copy
try_gen mkv_color_bt709.mkv "${VSRC[@]}" \
  -vf "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" "${H264[@]}"
try_gen mkv_color_bt2020_pq.mkv "${VSRC[@]}" \
  -vf "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc:range=tv" \
  -c:v libx265 -x265-params log-level=error -pix_fmt yuv420p10le
try_gen mkv_full_range.mkv "${VSRC[@]}" \
  -vf "setparams=color_primaries=smpte170m:color_trc=smpte170m:colorspace=smpte170m:range=pc" \
  "${H264[@]}"
try_gen mkv_vp9_10bit.mkv "${VSRC[@]}" "${VP9[@]}" -pix_fmt yuv420p10le
# 1024 samples a block. The pinned FFmpeg aborts on -frame_size with PCM;
# the filter cuts the same blocks, the last one short.
try_gen mkv_pcm.mkv "${ASRC[@]}" -af asetnsamples=n=1024:p=0 -c:a pcm_s16le
try_gen mkv_flac.mkv "${ASRC[@]}" -c:a flac
try_gen mkv_mp3.mkv "${ASRC[@]}" -c:a libmp3lame -b:a 128k
try_gen mkv_ac3.mkv "${ASRC[@]}" -c:a ac3
try_gen mkv_no_crc.mkv "${VSRC[@]}" "${H264[@]}" -write_crc32 0
printf '1\n00:00:00,000 --> 00:00:01,000\nhello\n\n2\n00:00:01,000 --> 00:00:02,000\nworld\n' > "$SRC/subs.srt"
try_gen mkv_multitrack.mkv \
  -f lavfi -i "testsrc2=size=320x240:rate=25:duration=2" \
  -f lavfi -i "testsrc2=size=160x120:rate=30:duration=2" \
  -f lavfi -i "sine=frequency=440:duration=2:sample_rate=48000" \
  -f lavfi -i "sine=frequency=880:duration=2:sample_rate=44100" \
  -i "$SRC/subs.srt" -i "$SRC/subs.srt" \
  -map 0:v -map 1:v -map 2:a -map 3:a -map 4:s -map 5:s \
  "${H264[@]}" -c:a aac -c:s:0 srt -c:s:1 ass \
  -metadata:s:a:0 language=eng -metadata:s:a:1 language=kor -metadata:s:s:0 language=eng \
  -metadata:s:v:1 title=small
try_gen webm_webvtt.webm "${VSRC[@]}" -i "$SRC/subs.srt" -map 0:v -map 1:s "${VP9[@]}" -c:s webvtt
printf 'not a real font\n' > "$SRC/font.ttf"
try_gen mkv_attachment.mkv -i "$OUT/mkv_h264_aac.mkv" -map 0 -c copy -attach "$SRC/font.ttf" \
  -metadata:s:t mimetype=application/x-truetype-font

# --- Crafted from the files above (scripts/mkv_craft.py) ---
craft mkv_lace_xiph.webm webm_vp9_opus.webm lace xiph --track 2 --group 4
craft mkv_lace_ebml.webm webm_vp9_opus.webm lace ebml --track 2 --group 5
craft mkv_lace_fixed.mkv mkv_pcm.mkv lace fixed --track 1 --group 3
craft mkv_header_strip.mkv mkv_h264_aac.mkv strip --track 1 --bytes 4
craft mkv_block_groups.mkv mkv_h264_aac.mkv groups --track 1
craft mkv_timestamp_scale.webm webm_vp9_opus.webm scale --timestamp-scale 500000
# Laced audio at 44.1 kHz, where a frame's DefaultDuration is not a whole
# millisecond: an AAC frame lasts its share of the block, an AC-3 frame
# what its header says (found black box). ffmpeg writes no DefaultDuration
# for AAC; this one is 1024 samples.
A44=(-f lavfi -i "sine=frequency=440:duration=2:sample_rate=44100")
if "${FF[@]}" "${A44[@]}" -c:a aac "$SRC/aac_44k.mkv" 2> /dev/null \
    && "${FF[@]}" "${A44[@]}" -c:a ac3 "$SRC/ac3_44k.mkv" 2> /dev/null; then
  craft mkv_lace_aac_44k.mkv .sources/aac_44k.mkv lace ebml --track 1 --group 8 \
    --default-duration 23219954
  craft mkv_lace_ac3_44k.mkv .sources/ac3_44k.mkv lace ebml --track 1 --group 8
else
  skip mkv_lace_aac_44k.mkv "ffmpeg-failed"
  skip mkv_lace_ac3_44k.mkv "ffmpeg-failed"
fi
craft mkv_live_clusters.webm webm_vp9_opus.webm live

# Headers that understate what the blocks hold (docs/design/policy-matroska.md):
# PixelWidth and PixelHeight smaller than the VP8 key frames and than the
# H.264 SPS in CodecPrivate, and a Duration shorter than the blocks' span.
craft size_vp8_understated.webm webm_vp8_vorbis.webm pixels 160 120 --track 1
craft size_h264_understated.mkv mkv_h264_aac.mkv pixels 160 120 --track 1
craft mkv_duration_understated.mkv mkv_h264_aac.mkv duration 1.0
# H.264 as Chrome's MediaRecorder writes it: no CodecPrivate, start codes.
craft mkv_h264_annexb.mkv mkv_h264_aac.mkv annexb --track 1
craft size_h264_annexb_understated.mkv mkv_h264_annexb.mkv pixels 160 120 --track 1
# VP9 frames that grow from 320x240 to 1280x720 mid-stream; the track header
# states the first size.
JOIN=$(mktemp -d "$OUT/.join.XXXXXX")
if "${FF[@]}" -f lavfi -i "testsrc2=size=320x240:rate=25:duration=1" "${VP9[@]}" "$JOIN/a.ivf" 2> /dev/null \
    && "${FF[@]}" -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" "${VP9[@]}" "$JOIN/b.ivf" 2> /dev/null \
    && printf "file '%s'\nfile '%s'\n" "$JOIN/a.ivf" "$JOIN/b.ivf" > "$JOIN/list.txt" \
    && "${FF[@]}" -f concat -safe 0 -i "$JOIN/list.txt" -c copy "$OUT/size_vp9_resize.webm" \
      2> "$OUT/size_vp9_resize.webm.log"; then
  rm -f "$OUT/size_vp9_resize.webm.log"; generated size_vp9_resize.webm
else
  rm -f "$OUT/size_vp9_resize.webm"; skip size_vp9_resize.webm "join-failed"
fi
rm -rf "$JOIN"

# --- Truncation ---
SIZE=$(wc -c < "$OUT/mkv_h264_aac.mkv" | tr -d ' ')
head -c $((SIZE * 2 / 3)) "$OUT/mkv_h264_aac.mkv" > "$OUT/mkv_truncated.mkv"; generated mkv_truncated.mkv
SIZE=$(wc -c < "$OUT/webm_live.webm" | tr -d ' ')
head -c $((SIZE / 2)) "$OUT/webm_live.webm" > "$OUT/webm_live_truncated.webm"
generated webm_live_truncated.webm

rm -rf "$SRC"
echo "fixtures written to $OUT (see $OUT/MANIFEST)"
