#!/bin/bash
# nmb.sh: macroblocks a stream's probe run decodes (pictures up to the class frame count, times the coded size in
# macroblocks), from ffprobe: one line "STREAM NMB" a stream. ES (env) is the streams' directory.
for f in "$ES"/*.h264; do
  b=$(basename $f .h264); c=${b##*.}
  case $c in sd) n=1800;; 720p) n=900;; 1080p) n=300;; 4k) n=150;; esac
  read w h p < <(ffprobe -v error -select_streams v:0 -count_packets -show_entries stream=coded_width,coded_height,nb_read_packets -of csv=p=0 $f | tr ',' ' ')
  echo "$b $(( (p < n ? p : n) * ((w + 15) / 16) * ((h + 15) / 16) ))"
done
