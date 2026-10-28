#!/bin/sh
# cgs.sh BIN STREAM_PREFIX FRAMES : cachegrind with cache and branch simulation, --discard
ES=$HOME/h264work/train/es; f=$(ls $ES | grep ^$2)
valgrind --tool=cachegrind --cache-sim=yes --branch-sim=yes --cachegrind-out-file=$HOME/h264work/r25/cgs.$1.$2.out $HOME/h264work/bins/$1 --frames $3 --discard $ES/$f > $HOME/h264work/r25/cgs.$1.$2.log 2>&1
