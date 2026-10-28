#!/usr/bin/env bash
# Generate test fixtures with the ffmpeg binary (used as a black box only).
# Usage: scripts/gen-fixtures.sh /absolute/output/dir
#
# Writes $OUT/MANIFEST with one line per fixture: "generated NAME" or
# "skipped NAME REASON". Required fixtures fail by default; set
# COREAV_REQUIRE_ALL_FIXTURES=0 only for best-effort local generation.
# Best-effort fixtures may always be skipped; the oracle rejects required skips.
set -euo pipefail

OUT="${1:?usage: gen-fixtures.sh /absolute/output/dir}"
mkdir -p "$OUT"
MANIFEST="$OUT/MANIFEST"
: > "$MANIFEST"
REQUIRE_ALL="${COREAV_REQUIRE_ALL_FIXTURES:-1}"
SCRIPTS="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

FF=(ffmpeg -hide_banner -loglevel error -y)
SRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=2"
     -f lavfi -i "sine=frequency=440:duration=2")
VSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=2")
ASRC=(-f lavfi -i "sine=frequency=440:duration=2:sample_rate=48000")

# Capture the encoder list once. Piping ffmpeg into `grep -q` can fail under
# `set -o pipefail`: grep exits early, ffmpeg gets SIGPIPE, and the pipeline
# reports failure even when the encoder exists.
ENCODERS="$(ffmpeg -hide_banner -encoders 2>/dev/null || true)"

if grep -q libx264 <<< "$ENCODERS"; then
  VID=(-c:v libx264 -pix_fmt yuv420p -g 25)
else
  VID=(-c:v mpeg4 -g 25)
fi
AUD=(-c:a aac -shortest)

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

# try_gen_best_effort NAME FFMPEG_ARGS...   (never required)
try_gen_best_effort() {
  local name="$1"; shift
  if "${FF[@]}" "$@" "$OUT/$name" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "ffmpeg-failed(see-$name.log)" best-effort
  fi
}

# --- Basic containers and truncation ---
"${FF[@]}" "${SRC[@]}" "${VID[@]}" "${AUD[@]}" "$OUT/basic.mp4"; generated basic.mp4
"${FF[@]}" "${SRC[@]}" "${VID[@]}" "${AUD[@]}" -movflags +faststart "$OUT/faststart.mp4"; generated faststart.mp4
"${FF[@]}" "${SRC[@]}" "${VID[@]}" "${AUD[@]}" -movflags frag_keyframe+empty_moov "$OUT/fragmented.mp4"; generated fragmented.mp4
# Fragmented, first fragment kept in the movie box: header durations cover only
# those samples.
"${FF[@]}" "${SRC[@]}" "${VID[@]}" "${AUD[@]}" -movflags frag_keyframe "$OUT/fragmented_moov.mp4"; generated fragmented_moov.mp4
"${FF[@]}" "${SRC[@]}" "${VID[@]}" "${AUD[@]}" "$OUT/basic.mov"; generated basic.mov

# Explicit timing cases: irregular presentation intervals, signed composition
# offsets with reordered frames, and an initial empty edit before media.
try_gen vfr.mp4 "${VSRC[@]}" -vf "select='if(lt(t,1),1,not(mod(n,3)))'" \
  -fps_mode vfr "${VID[@]}"
try_gen negative_cts.mp4 "${VSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -bf 3 -g 25 -movflags +negative_cts_offsets
try_gen edit_list.mp4 -itsoffset 0.5 -i "$OUT/basic.mp4" -map 0 -c copy -use_editlist 1
try_gen browser.webm "${SRC[@]}" -c:v libvpx-vp9 -deadline realtime \
  -cpu-used 8 -c:a libopus -shortest

SIZE=$(wc -c < "$OUT/basic.mp4" | tr -d ' ')
head -c $((SIZE - 100)) "$OUT/basic.mp4" > "$OUT/truncated.mp4"; generated truncated.mp4

# --- Rotation. Stream copy, so frames are not re-encoded or auto-rotated.
# -display_rotation is counter-clockwise (ffmpeg >= 6.0).
for deg in 90 180 270; do
  try_gen "rotate_ccw$deg.mp4" -display_rotation:v:0 "$deg" -i "$OUT/basic.mp4" -map 0 -c copy
done

# --- Color (colr nclx). setparams tags the frames; encoder options alone
# do not reliably set primaries and transfer in every ffmpeg version. ---
try_gen color_bt709.mp4 "${VSRC[@]}" \
  -vf "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" \
  "${VID[@]}" -movflags +write_colr
try_gen color_full_range.mp4 "${VSRC[@]}" \
  -vf "setparams=color_primaries=smpte170m:color_trc=smpte170m:colorspace=smpte170m:range=pc" \
  "${VID[@]}" -movflags +write_colr
try_gen color_bt2020_pq.mp4 "${VSRC[@]}" \
  -vf "setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc:range=tv" \
  -c:v libx265 -x265-params log-level=error -pix_fmt yuv420p10le -tag:v hvc1 -movflags +write_colr

# --- Colour precedence (docs/design/probe-color.md): a colr box next to a
# parameter set that states more, less or something else. The box is set
# with scripts/colr_craft.py; the parameter set with the encoder's options
# or h264_metadata. Sources are not fixtures themselves. ---
colr_entry() { # SOURCE NAME COLR_ARGS...
  local src="$OUT/$1" name="$2"; shift 2
  if [ ! -f "$src" ]; then
    skip "$name" "missing-source"
  elif python3 -I "$SCRIPTS/colr_craft.py" "$src" "$OUT/$name" "$@" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "colr-craft-failed(see-$name.log)"
  fi
}
drop_source() { rm -f "$OUT/$1"; sed -i "/^generated $1\$/d; /^skipped $1 /d" "$MANIFEST"; }
CSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=1")
try_gen color_src_novui.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p
try_gen color_src_unspecified.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -bsf:v h264_metadata=colour_primaries=2:transfer_characteristics=2:matrix_coefficients=2
try_gen color_src_range_pc.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p -color_range pc
try_gen color_src_hevc.mp4 "${CSRC[@]}" -c:v libx265 -x265-params log-level=error -tag:v hvc1
# H.264 without a colour description: the box's values.
colr_entry color_src_novui.mp4 color_colr_only.mp4 nclx 1 1 1 0
# H.264 whose SPS says unspecified (2): the SPS's, so nothing.
colr_entry color_src_unspecified.mp4 color_sps_unspecified.mp4 nclx 1 1 1 0
# H.264 whose SPS signals only the range: its range, the box's colours.
colr_entry color_src_range_pc.mp4 color_sps_range_only.mp4 nclx 1 1 1 0
# HEVC whose SPS signals a limited range and no colour description, under a
# full-range BT.709 box: the SPS's, the box ignored.
colr_entry color_src_hevc.mp4 color_hevc_colr_ignored.mp4 nclx 1 1 1 1
# H.264 whose SPS says SMPTE 170M under a BT.709 box: the SPS's.
try_gen color_colr_vs_sps.mp4 "${CSRC[@]}" \
  -vf "setparams=color_primaries=bt709:color_trc=bt709:colorspace=bt709:range=tv" \
  -c:v libx264 -pix_fmt yuv420p -movflags +write_colr \
  -bsf:v h264_metadata=colour_primaries=6:transfer_characteristics=6:matrix_coefficients=6
for src in color_src_novui.mp4 color_src_unspecified.mp4 color_src_range_pc.mp4 color_src_hevc.mp4; do
  drop_source "$src"
done

# --- Edit lists with several entries (docs/design/edit-lists.md): files
# whose edit list scripts/elst_craft.py replaces. Durations are in movie
# ticks (1/1000 s), media times in media ticks (600 Hz video, 44.1 kHz
# audio); 30 frames per second, a sync sample every 30 frames. Sources are
# not fixtures themselves. ---
elst_entry() { # SOURCE NAME TRACK EDIT...
  local src="$OUT/$1" name="$2"; shift 2
  if [ ! -f "$src" ]; then
    skip "$name" "missing-source"
  elif python3 -I "$SCRIPTS/elst_craft.py" "$src" "$OUT/$name" "$@" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "elst-craft-failed(see-$name.log)"
  fi
}
# Fields the pinned ffmpeg does not write, patched in place by
# scripts/iso_patch.py (no box changes size).
iso_patch() { # SOURCE NAME OPERATION VALUE...
  local src="$OUT/$1" name="$2"; shift 2
  if [ ! -f "$src" ]; then
    skip "$name" "missing-source"
  elif python3 -I "$SCRIPTS/iso_patch.py" "$src" "$OUT/$name" "$@" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "iso-patch-failed(see-$name.log)"
  fi
}
ESRC=(-f lavfi -i "testsrc2=size=160x120:rate=30:duration=10")
EGOP=(-g 30 -keyint_min 30 -sc_threshold 0 -video_track_timescale 600 -pix_fmt yuv420p)
try_gen edit_src_h264.mp4 "${ESRC[@]}" -c:v libx264 -bf 0 "${EGOP[@]}"
try_gen edit_src_bframes.mp4 "${ESRC[@]}" -c:v libx264 -bf 2 "${EGOP[@]}"
try_gen edit_src_aac.mp4 -f lavfi -i "sine=d=10:r=44100" -c:a aac
# Two edits joined at a sync sample, skipping a second of media.
elst_entry edit_src_h264.mp4 edit_multi_splice.mp4 0 3000:0 3000:2400
# B-frames; the second edit starts inside a group of pictures.
elst_entry edit_src_bframes.mp4 edit_multi_bframes.mp4 0 3000:40 3000:2500
# An empty edit between two media edits.
elst_entry edit_src_h264.mp4 edit_multi_empty.mp4 0 4000:0 1000:-1 5000:2520
# Audio: three edits that start inside AAC frames.
elst_entry edit_src_aac.mp4 edit_multi_audio.mp4 0 4000:1024 3000:180000 2500:320000
# The same with its declared averages (`btrt` and `esds`) set to 12345:
# ffprobe takes the bytes over the duration, not the declared value.
iso_patch edit_multi_audio.mp4 edit_multi_audio_average.mp4 average 0 12345
# Video and audio, the audio with three edits (clips keep its windows).
# Without perceptual noise substitution: its noise is random, so audio
# decoded from a clip's restart would not be sample-exact against the
# source's (measured: about 1e-5 off from the second frame on).
try_gen edit_src_av.mp4 "${ESRC[@]}" -f lavfi -i "sine=d=10:r=44100" -map 0:v -map 1:a \
  -c:v libx264 -bf 0 "${EGOP[@]}" -c:a aac -aac_pns 0
elst_entry edit_src_av.mp4 edit_multi_av.mp4 1 4000:1024 3000:180000 2500:320000
# One media edit that presents 6.5 s of 10: CoreAV's bit rate counts every
# sample over the presented time (KD-P-EDITED-BIT-RATE).
elst_entry edit_src_aac.mp4 edit_trim_audio.mp4 0 6500:1024
for src in edit_src_h264.mp4 edit_src_bframes.mp4 edit_src_aac.mp4 edit_src_av.mp4; do
  drop_source "$src"
done

# --- Video codecs ---
try_gen hevc_hev1.mp4 "${VSRC[@]}" -c:v libx265 -x265-params log-level=error
try_gen hevc_hvc1.mp4 "${VSRC[@]}" -c:v libx265 -x265-params log-level=error -tag:v hvc1
try_gen_best_effort h264_avc3.mp4 "${VSRC[@]}" -c:v libx264 -pix_fmt yuv420p -tag:v avc3
AV1=""
for e in libsvtav1 libaom-av1 librav1e; do
  if grep -qw -- "$e" <<< "$ENCODERS"; then AV1="$e"; break; fi
done
if [ -n "$AV1" ]; then
  try_gen av1.mp4 "${VSRC[@]}" -c:v "$AV1" -pix_fmt yuv420p
  # AV1 whose sequence header has no colour description, under a box that
  # has one: the sequence header's (docs/design/probe-color.md).
  extra=()
  [ "$AV1" = libaom-av1 ] && extra=(-cpu-used 8)
  try_gen color_src_av1.mp4 "${CSRC[@]}" -c:v "$AV1" "${extra[@]}" -pix_fmt yuv420p
  colr_entry color_src_av1.mp4 color_av1_colr_ignored.mp4 nclx 6 6 6 1
  drop_source color_src_av1.mp4
else
  skip av1.mp4 "no-AV1-encoder"
  skip color_av1_colr_ignored.mp4 "no-AV1-encoder"
fi
try_gen vp9.mp4 "${VSRC[@]}" -c:v libvpx-vp9 -deadline realtime -cpu-used 8

# --- ProRes, one file per profile (prores_ks profiles 0..5) ---
i=0
for p in proxy lt standard hq 4444 4444xq; do
  case "$p" in 4444|4444xq) pf=yuva444p10le ;; *) pf=yuv422p10le ;; esac
  try_gen "prores_$p.mov" -f lavfi -i "testsrc2=size=320x240:rate=25:duration=1" \
    -c:v prores_ks -profile:v "$i" -pix_fmt "$pf"
  i=$((i + 1))
done

# --- Picture sizes the sample entry does not tell (docs/design/frame-sizes.md) ---
# Each file decodes to 1280x720 pictures, at least in part, while its sample
# entry says less.
# understate_entry SOURCE NAME FOURCC WIDTH HEIGHT: a copy whose visual
# sample entry (the last FOURCC in the file, inside the movie box) declares
# WIDTH x HEIGHT; its parameter sets and frames are unchanged.
understate_entry() {
  local src="$OUT/$1" out="$OUT/$2" at
  [ -f "$src" ] || { skip "$2" "no-source"; return; }
  cp "$src" "$out"
  at=$(grep -obUa "$3" "$out" | tail -1 | cut -d: -f1)
  # shellcheck disable=SC2059
  printf "$(printf '\\x%02x\\x%02x\\x%02x\\x%02x' $(($4 >> 8)) $(($4 & 255)) $(($5 >> 8)) $(($5 & 255)))" \
    | dd of="$out" bs=1 seek=$((at + 28)) conv=notrunc status=none
  generated "$2"
}
try_gen size_source_720p.mp4 -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" "${VID[@]}"
understate_entry size_source_720p.mp4 size_entry_understated.mp4 avc1 320 240
rm -f "$OUT/size_source_720p.mp4"; sed -i '/^generated size_source_720p.mp4$/d' "$MANIFEST"
understate_entry prores_hq.mov size_prores_understated.mov apch 160 120
# A left crop in the SPS (48 samples, clause 7.4.2.1.1: the output is columns
# 48 to 1279 of the coded 1280; FFmpeg 7 skips it unless asked for exact
# cropping, `-flags unaligned`): the cropped size 1232x720 in the sample entry.
try_gen size_left_crop_source.mp4 -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" \
  "${VID[@]}" -bsf:v h264_metadata=crop_left=48
understate_entry size_left_crop_source.mp4 size_left_crop.mp4 avc1 1232 720
rm -f "$OUT/size_left_crop_source.mp4"; sed -i '/^generated size_left_crop_source.mp4$/d' "$MANIFEST"
# An HEVC conformance window that leaves nothing (left 1250 + right 40 of
# 1280): decoders ignore it as a whole; the entry says 1240x720.
try_gen size_window_source.mp4 -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" \
  -c:v libx265 -x265-params log-level=error -tag:v hvc1 \
  -bsf:v hevc_metadata=crop_left=1250:crop_right=40
understate_entry size_window_source.mp4 size_hevc_window.mp4 hvc1 1240 720
rm -f "$OUT/size_window_source.mp4"; sed -i '/^generated size_window_source.mp4$/d' "$MANIFEST"
# A window invalid in width (left 1250 + right 40 of 1280) with a valid
# bottom padding crop (30): ignored in both dimensions; the entry says
# 1280x690.
try_gen size_mixed_source.mp4 -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" \
  -c:v libx265 -x265-params log-level=error -tag:v hvc1 \
  -bsf:v hevc_metadata=crop_left=1250:crop_right=40:crop_bottom=30
understate_entry size_mixed_source.mp4 size_hevc_mixed_window.mp4 hvc1 1280 690
rm -f "$OUT/size_mixed_source.mp4"; sed -i '/^generated size_mixed_source.mp4$/d' "$MANIFEST"
# Two coded video sequences of different sizes in one track: the second's
# parameter sets (H.264) or key frame (VP9) are in the samples, the record
# and the sample entry describe the first.
size_join() { # NAME FORMAT CODEC_ARGS...
  local name="$1" format="$2" dir; shift 2
  dir=$(mktemp -d "$OUT/.join.XXXXXX")
  if "${FF[@]}" -f lavfi -i "testsrc2=size=320x240:rate=25:duration=1" "$@" "$dir/a.$format" 2> /dev/null \
    && "${FF[@]}" -f lavfi -i "testsrc2=size=1280x720:rate=25:duration=1" "$@" "$dir/b.$format" 2> /dev/null; then
    if [ "$format" = h264 ]; then
      cat "$dir/a.h264" "$dir/b.h264" > "$dir/ab.h264"
      "${FF[@]}" -f h264 -r 25 -i "$dir/ab.h264" -c copy "$OUT/$name" 2> "$OUT/$name.log"
    else
      printf "file '%s'\nfile '%s'\n" "$dir/a.$format" "$dir/b.$format" > "$dir/list.txt"
      "${FF[@]}" -f concat -safe 0 -i "$dir/list.txt" -c copy "$OUT/$name" 2> "$OUT/$name.log"
    fi && rm -f "$OUT/$name.log" && generated "$name" || skip "$name" "join-failed(see-$name.log)"
  else
    skip "$name" "ffmpeg-failed"
  fi
  rm -rf "$dir"
}
size_join size_inband_sps.mp4 h264 -c:v libx264 -pix_fmt yuv420p -bsf:v h264_mp4toannexb
size_join size_vp9_resize.mp4 ivf -c:v libvpx-vp9 -deadline realtime -cpu-used 8

# --- Audio ---
try_gen audio_aac.m4a "${ASRC[@]}" -c:a aac -b:a 128k
try_gen audio_51.m4a "${ASRC[@]}" -af "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0" -c:a aac
try_gen audio_opus.mp4 "${ASRC[@]}" -c:a libopus -b:a 64k
try_gen audio_mp3.mp4 "${ASRC[@]}" -c:a libmp3lame -b:a 128k
# QuickTime's own MP3 sound sample entry, `.mp3` (#57).
try_gen audio_mp3.mov "${ASRC[@]}" -c:a libmp3lame -b:a 128k
# AAC whose channels are in a program config element (channelConfiguration
# 0; ffmpeg writes one for a hexagonal layout, 6 channels), under a sample
# entry that says 2 (#56): a copy with the mp4a channelcount (20 bytes after
# the entry type) rewritten.
try_gen audio_pce_source.m4a "${ASRC[@]}" -af "aformat=channel_layouts=hexagonal" -c:a aac
if [ -f "$OUT/audio_pce_source.m4a" ]; then
  cp "$OUT/audio_pce_source.m4a" "$OUT/audio_pce_entry_stereo.m4a"
  at=$(grep -obUa mp4a "$OUT/audio_pce_entry_stereo.m4a" | tail -1 | cut -d: -f1)
  printf '\x00\x02' | dd of="$OUT/audio_pce_entry_stereo.m4a" bs=1 seek=$((at + 20)) conv=notrunc status=none
  generated audio_pce_entry_stereo.m4a
fi
rm -f "$OUT/audio_pce_source.m4a"; sed -i '/^generated audio_pce_source.m4a$/d' "$MANIFEST"
try_gen pcm_s16le.mov "${ASRC[@]}" -c:a pcm_s16le
try_gen pcm_s24le.mov "${ASRC[@]}" -c:a pcm_s24le
try_gen pcm_f32le.mov "${ASRC[@]}" -c:a pcm_f32le
try_gen_best_effort pcm_s16le.mp4 "${ASRC[@]}" -c:a pcm_s16le
try_gen pcm_s8_mono.mov "${ASRC[@]}" -c:a pcm_s8
try_gen pcm_u8_stereo.mov "${ASRC[@]}" -ac 2 -c:a pcm_u8
try_gen pcm_s16le_stereo.mov "${ASRC[@]}" -ac 2 -c:a pcm_s16le

# QuickTime uncompressed audio with a constant sample size of 1 in stsz:
# a copy of a file above with stsz rewritten to count frames of "1 byte"
# (FRAMES) or bytes (BYTES). The single stsz box is the last one in the
# file, inside the movie box after the media data.
legacy_pcm() { # SOURCE NAME frames|bytes FRAME_BYTES
  local src="$OUT/$1" out="$OUT/$2" mode="$3" frame="$4" at count
  [ -f "$src" ] || { skip "$2" "no-source"; return; }
  cp "$src" "$out"
  at=$(grep -obUa stsz "$out" | tail -1 | cut -d: -f1)
  count=$(od -An -t u4 --endian=big -j $((at + 12)) -N 4 "$out" | tr -d ' ')
  [ "$mode" = bytes ] && count=$((count * frame))
  be32() { printf '\\x%02x\\x%02x\\x%02x\\x%02x' $((($1 >> 24) & 255)) $((($1 >> 16) & 255)) $((($1 >> 8) & 255)) $(($1 & 255)); }
  # shellcheck disable=SC2059
  printf "$(be32 1)$(be32 "$count")" | dd of="$out" bs=1 seek=$((at + 8)) conv=notrunc status=none
  generated "$2"
}
legacy_pcm pcm_u8_stereo.mov pcm_u8_stereo_size1_frames.mov frames 2
legacy_pcm pcm_s16le_stereo.mov pcm_s16le_stereo_size1_bytes.mov bytes 4

# --- Several tracks (2 video, 2 audio with languages, 1 subtitle) ---
printf '1\n00:00:00,000 --> 00:00:01,000\nhello\n\n2\n00:00:01,000 --> 00:00:02,000\nworld\n' > "$OUT/subs.srt"
try_gen multitrack.mp4 \
  -f lavfi -i "testsrc2=size=320x240:rate=25:duration=2" \
  -f lavfi -i "testsrc2=size=160x120:rate=30:duration=2" \
  -f lavfi -i "sine=frequency=440:duration=2:sample_rate=48000" \
  -f lavfi -i "sine=frequency=880:duration=2:sample_rate=44100" \
  -i "$OUT/subs.srt" \
  -map 0:v -map 1:v -map 2:a -map 3:a -map 4:s \
  "${VID[@]}" -c:a aac -c:s mov_text \
  -metadata:s:a:0 language=eng -metadata:s:a:1 language=kor -metadata:s:s:0 language=eng
rm -f "$OUT/subs.srt"

# --- Text (docs/design/text.md): 3GPP timed text with styling, non-ASCII
# text, characters WebVTT escapes, line breaks and gaps; a subtitle track
# shifted by an edit; and QuickTime chapter tracks in MP4, M4A and MOV. The
# MP4 muxer also writes a Nero `chpl` box next to the chapter track. ---
printf '1\r\n00:00:00,250 --> 00:00:00,900\r\nPlain <b>bold</b> <i>italic</i> <u>under</u>\r\n\r\n2\r\n00:00:01,000 --> 00:00:01,600\r\nTwo\r\nlines & more --> here\r\n\r\n3\r\n00:00:01,600 --> 00:00:02,000\r\n<font color="#ff0000">Red</font> 한국어 ünï\r\n' > "$OUT/styled.srt"
printf ';FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=800\ntitle=Opening\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=800\nEND=1500\ntitle=Middle & <part>\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=2000\ntitle=Fin – 終わり\n' > "$OUT/chapters.txt"
try_gen text_styled.mp4 "${SRC[@]}" -i "$OUT/styled.srt" -map 0:v -map 1:a -map 2:s \
  "${VID[@]}" "${AUD[@]}" -c:s mov_text -metadata:s:s:0 language=kor
try_gen text_shifted.mp4 "${VSRC[@]}" -itsoffset 0.5 -i "$OUT/styled.srt" -map 0:v -map 1:s \
  "${VID[@]}" -c:s mov_text
# A text track whose track header is 400x60, which ffprobe reports as its
# size; the same without the text track's edit list, which ffprobe extends
# to the video's end (KD-TEXT-END).
try_gen text_sized.mp4 "${ESRC[@]}" -i "$OUT/styled.srt" -map 0:v -map 1:s \
  -c:v libx264 -pix_fmt yuv420p -c:s mov_text -s:s 400x60
elst_entry text_sized.mp4 text_end.mp4 1
# Fragmented with a text track (its bit rate is over its own span); and
# video with every fragment's decode time 2 s later, so the track starts
# late and lasts until its last sample ends (docs/design/compat-cli.md,
# stream bit rates). Video only, the M4A extraction refusing a fragmented
# AAC track that starts late; without B-frames, whose late start the remux
# round trip does not keep presented sample for sample.
try_gen fragmented_text.mp4 "${SRC[@]}" -i "$OUT/styled.srt" -map 0:v -map 1:a -map 2:s \
  "${VID[@]}" "${AUD[@]}" -c:s mov_text -movflags frag_keyframe+empty_moov+default_base_moof
try_gen fragmented_late_src.mp4 "${VSRC[@]}" "${VID[@]}" -bf 0 \
  -movflags frag_keyframe+empty_moov+default_base_moof
iso_patch fragmented_late_src.mp4 fragmented_late.mp4 tfdt 2
drop_source fragmented_late_src.mp4
try_gen chapters.mp4 "${SRC[@]}" -i "$OUT/styled.srt" -i "$OUT/chapters.txt" \
  -map 0:v -map 1:a -map 2:s -map_chapters 3 "${VID[@]}" "${AUD[@]}" -c:s mov_text
try_gen chapters.m4a "${ASRC[@]}" -i "$OUT/chapters.txt" -map 0:a -map_chapters 1 -c:a aac
try_gen chapters.mov "${VSRC[@]}" -i "$OUT/chapters.txt" -map 0:v -map_chapters 1 "${VID[@]}"
rm -f "$OUT/styled.srt" "$OUT/chapters.txt"
# chapters.mp4 with every box of one type renamed to `free` (ISO/IEC
# 14496-12 8.1.2), in place: without its `chap` track references (only the
# Nero `chpl` box is left) and without its `chpl` box (only the chapter
# track is left).
rename_boxes() { # SOURCE NAME TYPE
  local src="$OUT/$1" out="$OUT/$2" at
  [ -f "$src" ] || { skip "$2" "no-source"; return; }
  cp "$src" "$out"
  local size count=0
  for at in $(grep -obUa "$3" "$out" | cut -d: -f1); do
    # Only a box header: a size of 8 to 4,096 bytes in front of the type.
    # The type bytes can also occur inside sample data.
    [ "$at" -ge 4 ] || continue
    size=$(od -An -t u4 --endian=big -j $((at - 4)) -N 4 "$out" | tr -d ' ')
    [ "$size" -ge 8 ] && [ "$size" -le 4096 ] || continue
    printf 'free' | dd of="$out" bs=1 seek="$at" conv=notrunc status=none
    count=$((count + 1))
  done
  [ "$count" -gt 0 ] || { skip "$2" "no-$3-box"; return; }
  generated "$2"
}
rename_boxes chapters.mp4 chapters_chpl_only.mp4 tref
rename_boxes chapters.mp4 chapters_track_only.mp4 chpl
# text_sized.mp4 with its 400x60 timed text track named by the video's
# `chap` reference, as a podcast's chapter titles: ffprobe lists it as
# `bin_data` with no size, so CoreAV's compatible output gives none
# (KD-CHAPTER-TEXT covers its kind, codec and packets, not its size).
if [ ! -f "$OUT/text_sized.mp4" ]; then
  skip chapters_sized.mp4 missing-source
elif python3 -I "$SCRIPTS/chap_craft.py" "$OUT/text_sized.mp4" "$OUT/chapters_sized.mp4" 0 2 \
    2> "$OUT/chapters_sized.mp4.log"; then
  rm -f "$OUT/chapters_sized.mp4.log"; generated chapters_sized.mp4
else
  rm -f "$OUT/chapters_sized.mp4"; skip chapters_sized.mp4 "chap-craft-failed(see-chapters_sized.mp4.log)"
fi

# --- Timecode track ---
try_gen timecode.mov "${VSRC[@]}" "${VID[@]}" -timecode 01:00:00:00

# --- Clips: several GOPs, open GOPs, and audio or
# video that starts after the other stream. ---
CSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=4"
      -f lavfi -i "sine=frequency=440:duration=4:sample_rate=48000")
# Without B-pyramid, x264's open GOPs put leading B frames after each
# keyframe in decode order (black-box check with ffprobe's packet list).
try_gen clip_open_gop.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=25:min-keyint=25:scenecut=0:open-gop=1:bframes=3:b-pyramid=none "${AUD[@]}"
try_gen clip_hevc_open_gop.mp4 "${CSRC[@]}" -c:v libx265 -tag:v hvc1 \
  -x265-params log-level=error:keyint=25:min-keyint=25:scenecut=0:open-gop=1 "${AUD[@]}"
# One stream delayed by 0.3 s, which the muxer writes as a leading empty
# edit: the video of clip_open_gop.mp4 (stream copy) with 3 s of audio that
# ends before it, and that video delayed against its own audio.
if [ -f "$OUT/clip_open_gop.mp4" ]; then
  try_gen clip_audio_late.mp4 -i "$OUT/clip_open_gop.mp4" \
    -itsoffset 0.3 -f lavfi -i "sine=frequency=440:duration=3:sample_rate=48000" \
    -map 0:v -map 1:a -c:v copy -c:a aac
  try_gen clip_video_late.mp4 -itsoffset 0.3 -i "$OUT/clip_open_gop.mp4" -i "$OUT/clip_open_gop.mp4" \
    -map 0:v -map 1:a -c copy -use_editlist 1
else
  skip clip_audio_late.mp4 "needs-clip_open_gop.mp4"
  skip clip_video_late.mp4 "needs-clip_open_gop.mp4"
fi

# --- Cut points (docs/design/cut-points.md): H.264 and HEVC whose sync
# sample table is absent (every sample then a sync sample) or lists every
# frame, so that clip and split must read the coded data to cut on a
# decodable frame. The table is set with scripts/stss_craft.py; sources are
# not fixtures. The AV1 ones follow split_av1.mp4 below. ---
stss_entry() { # SOURCE NAME none|all
  local src="$OUT/$1" name="$2"
  if [ ! -f "$src" ]; then
    skip "$name" "missing-source"
  elif python3 -I "$SCRIPTS/stss_craft.py" "$src" "$OUT/$name" "$3" 2> "$OUT/$name.log"; then
    rm -f "$OUT/$name.log"; generated "$name"
  else
    rm -f "$OUT/$name"; skip "$name" "stss-craft-failed(see-$name.log)"
  fi
}
try_gen cut_src_single_ref.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=25:min-keyint=25:scenecut=0:bframes=0:ref=1 "${AUD[@]}"
try_gen cut_src_bframes.mp4 "${CSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=25:min-keyint=25:scenecut=0:bframes=2:ref=3 "${AUD[@]}"
# A white flash at 2 s that x264 codes as non-IDR I frames (a scene cut
# within its minimum key interval) in a stream of 16 reference frames: an
# intra frame that later P frames reach past. And HEVC with P pictures.
try_gen cut_src_flash.mp4 "${CSRC[@]}" \
  -vf "drawbox=x=0:y=0:w=iw:h=ih:color=white:t=fill:enable='between(t,2,2.03)'" \
  -c:v libx264 -pix_fmt yuv420p -x264-params keyint=250:min-keyint=250:scenecut=40:ref=16:bframes=0 "${AUD[@]}"
try_gen cut_src_hevc.mp4 "${CSRC[@]}" -c:v libx265 -tag:v hvc1 \
  -x265-params log-level=error:keyint=25:min-keyint=25:scenecut=0:open-gop=0 "${AUD[@]}"
stss_entry cut_src_single_ref.mp4 clip_no_stss.mp4 none
stss_entry cut_src_bframes.mp4 clip_no_stss_bframes.mp4 none
stss_entry cut_src_bframes.mp4 clip_stss_all.mp4 all
stss_entry cut_src_flash.mp4 clip_stss_all_flash.mp4 all
stss_entry cut_src_hevc.mp4 clip_hevc_stss_all.mp4 all
drop_source cut_src_single_ref.mp4
drop_source cut_src_bframes.mp4
drop_source cut_src_flash.mp4
drop_source cut_src_hevc.mp4

# --- Splits (docs/design/split.md): closed GOPs of 12 frames in every video
# codec the clip writer accepts, so that each is cut several times. ---
SSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=4"
      -f lavfi -i "sine=frequency=440:duration=4:sample_rate=48000")
try_gen split_hevc.mp4 "${SSRC[@]}" -c:v libx265 -tag:v hvc1 \
  -x265-params log-level=error:keyint=12:min-keyint=12:scenecut=0:open-gop=0 "${AUD[@]}"
if [ -n "$AV1" ]; then
  try_gen split_av1.mp4 "${SSRC[@]}" -c:v "$AV1" -pix_fmt yuv420p -g 12 "${AUD[@]}"
  # AV1 whose sync sample table lists every sample, and none: clip and split
  # read the coded data to cut on a key frame, and ffprobe flags the key
  # frames of the bitstream either way (docs/design/random-access.md).
  stss_entry split_av1.mp4 clip_av1_stss_all.mp4 all
  stss_entry split_av1.mp4 clip_av1_no_stss.mp4 none
else
  skip split_av1.mp4 "no-AV1-encoder"
  skip clip_av1_stss_all.mp4 "no-AV1-encoder"
  skip clip_av1_no_stss.mp4 "no-AV1-encoder"
fi
try_gen split_vp9.mp4 "${SSRC[@]}" -c:v libvpx-vp9 -deadline realtime -cpu-used 8 \
  -g 12 -keyint_min 12 "${AUD[@]}"

# --- HLS (docs/design/hls.md): long enough for several segments of the
# default 6 s target. Closed GOPs of 2 s with B-frames and B-pyramid,
# open GOPs, HEVC, and keyframes 5 s apart, longer than half the target.
HSRC=(-f lavfi -i "testsrc2=size=320x240:rate=25:duration=20"
      -f lavfi -i "sine=frequency=440:duration=20:sample_rate=48000")
try_gen hls_h264.mp4 "${HSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=50:min-keyint=50:scenecut=0:bframes=3:b-pyramid=normal "${AUD[@]}"
try_gen hls_open_gop.mp4 "${HSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=50:min-keyint=50:scenecut=0:open-gop=1:bframes=3:b-pyramid=none "${AUD[@]}"
try_gen hls_hevc.mp4 "${HSRC[@]}" -c:v libx265 -tag:v hvc1 \
  -x265-params log-level=error:keyint=50:min-keyint=50:scenecut=0 "${AUD[@]}"
try_gen hls_long_gop.mp4 "${HSRC[@]}" -c:v libx264 -pix_fmt yuv420p \
  -x264-params keyint=125:min-keyint=125:scenecut=0 "${AUD[@]}"

# --- 10,000 nested boxes: ftyp, then moov > trak > trak > ... (9,999 trak) ---
gen_nested() { # OUT_FILE LEVELS
  local out="$1" levels="$2" i size hex
  {
    printf '\x00\x00\x00\x10ftypisom\x00\x00\x02\x00'
    for ((i = 1; i <= levels; i++)); do
      size=$((8 * (levels - i + 1)))
      printf -v hex '\\x%02x\\x%02x\\x%02x\\x%02x' \
        $(((size >> 24) & 255)) $(((size >> 16) & 255)) $(((size >> 8) & 255)) $((size & 255))
      # shellcheck disable=SC2059
      printf "$hex"
      if [ "$i" -eq 1 ]; then printf 'moov'; else printf 'trak'; fi
    done
  } > "$out"
  local expected=$((16 + 8 * levels)) actual
  actual=$(wc -c < "$out" | tr -d ' ')
  [ "$actual" -eq "$expected" ] || { echo "nested fixture has $actual bytes, expected $expected" >&2; exit 1; }
}
gen_nested "$OUT/nested_10000.mp4" 10000; generated nested_10000.mp4

# --- Full-file remux and sanitization (docs/design/remux.md): location,
# device and creation metadata as QuickTime keys (MOV) and user data (MP4). ---
META=(-metadata "title=CoreAV fixture" -metadata creation_time=2026-01-02T03:04:05Z
      -metadata:s:a:0 language=kor)
try_gen remux_metadata.mov "${SRC[@]}" "${VID[@]}" "${AUD[@]}" "${META[@]}" \
  -metadata com.apple.quicktime.location.ISO6709=+37.5665+126.9780+010.000/ \
  -metadata com.apple.quicktime.make=CoreAV -metadata com.apple.quicktime.model=Fixture \
  -movflags use_metadata_tags
try_gen remux_metadata.mp4 "${SRC[@]}" "${VID[@]}" "${AUD[@]}" "${META[@]}" \
  -metadata location=+37.5665+126.9780/

echo "fixtures written to $OUT (see $OUT/MANIFEST)"

# --- Fragmented files for the remote index path. Long enough
# that one window per fragment stays well under 5% of the object. ---
FSRC=(-f lavfi -i "testsrc2=size=640x360:rate=25:duration=20"
      -f lavfi -i "sine=frequency=440:duration=20")
# One leading segment index per track, covering every fragment. Without
# default_base_moof the pinned ffmpeg writes absolute fragment base offsets
# and then inserts the index in front of them, so every sample position is
# off by the index size (black-box check: no decodable H.264 at them).
try_gen fragmented_sidx.mp4 "${FSRC[@]}" "${VID[@]}" "${AUD[@]}" \
  -movflags frag_keyframe+empty_moov+default_base_moof+global_sidx
# Heavily fragmented, no segment index: routed to download-then-index.
try_gen fragmented_many.mp4 "${FSRC[@]}" "${VID[@]}" -g 5 "${AUD[@]}" \
  -movflags frag_keyframe+empty_moov
