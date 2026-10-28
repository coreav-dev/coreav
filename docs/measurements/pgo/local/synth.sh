#!/bin/bash
# synth.sh OUT: synthetic H.264 training streams from lavfi sources, FFmpeg 9.0.2's libx264, one encoder thread.
set -e
F=/opt/coreav-oracle-bin/ffmpeg; O=$1
e() { n=$1; shift; $F -hide_banner -loglevel error -y "$@" -pix_fmt yuv420p -threads 1 -an -f h264 $O/$n.h264; }
e uhd-high  -f lavfi -i testsrc2=size=3840x2160:rate=30 -frames:v 40 -c:v libx264 -preset fast -crf 22
e fhd-high  -f lavfi -i mandelbrot=size=1920x1080:rate=30 -frames:v 90 -c:v libx264 -preset medium -crf 20
e fhd-noise -f lavfi -i testsrc2=size=1920x1080:rate=30 -frames:v 60 -vf noise=alls=12:allf=t -c:v libx264 -preset fast -crf 24 -bf 3
e hd-base   -f lavfi -i testsrc2=size=1280x720:rate=30 -frames:v 120 -vf noise=alls=8:allf=t -c:v libx264 -profile:v baseline -preset fast -crf 23
e hd-main   -f lavfi -i life=size=1280x720:rate=30:mold=10:ratio=0.5 -frames:v 120 -c:v libx264 -profile:v main -preset fast -crf 23
e sd-int    -f lavfi -i testsrc2=size=720x480:rate=30 -frames:v 120 -vf noise=alls=8:allf=t -c:v libx264 -preset fast -crf 22 -flags +ildct+ilme -x264-params interlaced=1
