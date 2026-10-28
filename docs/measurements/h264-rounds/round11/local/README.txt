Local runs behind figures that are information only (docs/design/h264.md 'Round 11'; docs/measurements/h264-rejected.md).
Machine: the 2.50 GHz class (Intel Xeon @ 2.50GHz), scripts/h264_parity.py --pin 3 --rounds 3 (f3) or 2 (thr), training set, frames 1800/900/300/150 per class.

f3.*.json: base = main 555689b, p = round 11 part P, m = part M, ml = M with the plane placement (rejected/h264-plane-placement on top of M), mpl = M, P and the placement. Summary: python3 fact.py . f3 ml/m mpl/p
ml/m     discard overall  n=32 1.0437 [1.0341,1.0573] faster 2/32
ml/m     discard 4k       n= 8 1.0777 [1.0569,1.1310] faster 0/8
ml/m     write   overall  n=32 1.0552 [1.0411,1.0651] faster 1/32
ml/m     write   4k       n= 8 1.0958 [1.0402,1.1908] faster 0/8
mpl/p    discard overall  n=32 1.0157 [1.0028,1.0298] faster 10/32
mpl/p    discard 4k       n= 8 1.0142 [0.9508,1.0482] faster 3/8
mpl/p    write   overall  n=32 1.0138 [1.0039,1.0233] faster 9/32
mpl/p    write   4k       n= 8 1.0281 [0.9822,1.0440] faster 2/8

thr.*.json: main 555689b as built (base) and with MALLOC_MMAP_THRESHOLD_ fixed at 131072 (t128k) and 1048576 (t1m), 1080p and 4k. A fixed threshold also maps and unmaps the per-picture record array (1 MB at 1080p, 4 MB at 4k) every picture, so these mix placement with that churn. Summary: python3 fact.py . thr t128k/base t1m/base
t128k/base discard overall  n=16 1.0136 [0.9781,1.0668] faster 6/16
t128k/base write   overall  n=16 1.0236 [1.0037,1.0460] faster 3/16
t1m/base discard overall  n=16 1.0243 [1.0092,1.0374] faster 2/16
t1m/base write   overall  n=16 1.0352 [0.9964,1.0550] faster 5/16
