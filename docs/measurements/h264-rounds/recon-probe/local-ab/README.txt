Local timing of k (d6e962f7, binary r33-ct2) against base (main 4f456984 code, binary m-b366429), built with
`cargo build --locked --profile dist --example decode -p coreav-h264` (rustc 1.99.1) on a development VM:
Intel(R) Xeon(R) Processor @ 2.90GHz, family 6, model 106, 8 logical CPUs.
Command: python3 -I ab3.py 6 <the 32 training stream ids> m-b366429 r33-ct2 > ab1.txt
(one thread, pinned to CPU 3, --discard, class frame counts sd 1800, 720p 900, 1080p 300, 4k 150, CPU time, order
rotated each round). ab1sum.txt: python3 -I ab1sum.py ab1.txt, from the per-stream ratios as ab1.txt prints them (four
decimals), so its median can differ from ab1.txt's last line in the fourth decimal.
