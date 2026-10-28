# instruction counts per binary for a few streams, short segments
for s in 278dbf5a63bb.0.720p 6e90422a670a.0.1080p 41e324ebb708.0.sd 81ed4778ece2.0.4k; do
  case $s in *sd) F=200;; *720p) F=60;; *1080p) F=30;; *4k) F=10;; esac
  line="$s"
  for b in base w k1 k2; do
    ir=$(valgrind --tool=callgrind --callgrind-out-file=/tmp/cg.$b.$s ~/pk/decode-$b --frames $F --discard ~/tr/es/$s.h264 2>&1 >/dev/null | grep -o "Collected : [0-9]*" | grep -o "[0-9]*")
    line="$line $b=$ir"
  done
  echo $line
done
