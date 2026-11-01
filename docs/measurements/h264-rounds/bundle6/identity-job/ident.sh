#!/bin/bash
# ident.sh SET STREAM THREADS VARIANT: the output of decode-k<V> against decode-base<V> on one stream, VARIANT fast
# (the default build) or nf (--no-default-features). Each decoder's exit status is checked, and an empty output is
# a failure; the first failure or difference writes out/identity.stop, and no later call starts.
set -u -o pipefail
set=$1; f=$2; t=$3; v=$4
[ -e out/identity.stop ] && exit 0
case $v in fast) sfx=;; nf) sfx=-nf;; *) echo "bad variant $v" >&2; touch out/identity.stop; exit 2;; esac
case $f in *.sd.h264) n=1800;; *.720p.h264) n=900;; *.1080p.h264) n=300;; *.4k.h264) n=150;; *) echo "bad stream $f" >&2; touch out/identity.stop; exit 2;; esac
s=$(basename $f .h264)
empty=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
run() { local h; h=$(./decode-$1$sfx --threads $t --frames $n $f 2>out/err.$s.t$t.$v.$1 | sha256sum) || return 1; h=${h%% *}; [ "$h" != $empty ] || return 1; echo $h; }
a=$(run base); ra=$?
b=$(run k); rb=$?
if [ $ra -ne 0 ] || [ $rb -ne 0 ]; then r=FAIL; elif [ "$a" != "$b" ]; then r=DIFF; else r=same; fi
echo "$set $s t$t $v base=${a:-none} k=${b:-none} $r" >> out/identity.txt
if [ $r != same ]; then touch out/identity.stop; exit 1; fi
rm -f out/err.$s.t$t.$v.base out/err.$s.t$t.$v.k
