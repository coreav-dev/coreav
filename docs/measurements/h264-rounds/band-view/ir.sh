#!/bin/bash
# ir.sh: instructions (callgrind Ir) at one thread, base and k, two streams.
for f in 175433794e02.0.1080p.h264 41e324ebb708.0.sd.h264; do
  for a in base k; do
    valgrind --tool=callgrind --callgrind-out-file=/dev/null ~/r5/w/dec-$a --frames 60 --discard ~/tr/es/$f 2>&1 | grep -o 'Collected : [0-9]*' | sed "s/^/$f $a /"
  done
done
