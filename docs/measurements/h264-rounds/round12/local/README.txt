Local runs (information only; docs/design/h264.md 'Round 12'). Machine: the 2.90 GHz class (Intel Xeon @ 2.90GHz, family 6 model 106 stepping 6, the bar machine's class), scripts/h264_parity.py --pin 3 --rounds 3, training set, frames 1800/900/300/150 per class.

onr11.*.json: one binary built from round 11's combined tip with the hints switched at run time (COREAV_EXP_PF): x0 none, x1 destination, x2 reference, x3 both. Summary: python3 fact.py . onr11 x1/x0 x2/x0 x3/x0 x3/x1 x3/x2
x1/x0    discard overall  n=32 0.9783 [0.9628,0.9983] faster 22/32
x1/x0    discard 4k       n= 8 0.9171 [0.8567,0.9855] faster 7/8
x1/x0    write   overall  n=32 0.9818 [0.9712,0.9949] faster 23/32
x1/x0    write   4k       n= 8 0.9538 [0.8839,0.9811] faster 8/8
x2/x0    discard overall  n=32 0.9882 [0.9767,0.9951] faster 24/32
x2/x0    discard 4k       n= 8 0.9762 [0.9523,0.9941] faster 8/8
x2/x0    write   overall  n=32 0.9935 [0.9843,1.0014] faster 21/32
x2/x0    write   4k       n= 8 0.9835 [0.9664,0.9937] faster 7/8
x3/x0    discard overall  n=32 0.9577 [0.9274,0.9925] faster 23/32
x3/x0    discard 4k       n= 8 0.8894 [0.8024,0.9685] faster 7/8
x3/x0    write   overall  n=32 0.9850 [0.9585,1.0033] faster 19/32
x3/x0    write   4k       n= 8 0.9325 [0.8480,0.9781] faster 7/8
x3/x1    discard overall  n=32 0.9828 [0.9729,0.9985] faster 23/32
x3/x1    discard 4k       n= 8 0.9754 [0.9294,0.9971] faster 7/8
x3/x1    write   overall  n=32 0.9901 [0.9756,0.9987] faster 22/32
x3/x1    write   4k       n= 8 0.9762 [0.9431,0.9970] faster 7/8
x3/x2    discard overall  n=32 0.9789 [0.9627,0.9958] faster 25/32
x3/x2    discard 4k       n= 8 0.9152 [0.8427,0.9779] faster 7/8
x3/x2    write   overall  n=32 0.9914 [0.9639,0.9997] faster 22/32
x3/x2    write   4k       n= 8 0.9411 [0.8679,0.9952] faster 7/8

parts.*.json: the same switch built on main e6d810c with round 12's size rule (720p, 1080p, 4k): m0 none, m1 destination, m2 reference, m3 both. Summary: python3 fact.py . parts m1/m0 m2/m0 m3/m0 m3/m1 m3/m2
m1/m0    discard overall  n=24 0.9858 [0.9320,0.9985] faster 17/24
m1/m0    discard 4k       n= 8 0.8744 [0.8225,0.9959] faster 7/8
m1/m0    write   overall  n=24 0.9817 [0.9512,0.9932] faster 20/24
m1/m0    write   4k       n= 8 0.9004 [0.8619,0.9691] faster 8/8
m2/m0    discard overall  n=24 0.9837 [0.9478,0.9955] faster 19/24
m2/m0    discard 4k       n= 8 0.9394 [0.8789,0.9733] faster 8/8
m2/m0    write   overall  n=24 0.9828 [0.9682,0.9907] faster 18/24
m2/m0    write   4k       n= 8 0.9564 [0.9226,0.9829] faster 7/8
m3/m0    discard overall  n=24 0.9707 [0.9164,0.9880] faster 22/24
m3/m0    discard 4k       n= 8 0.8004 [0.7458,0.9548] faster 8/8
m3/m0    write   overall  n=24 0.9629 [0.9362,0.9815] faster 21/24
m3/m0    write   4k       n= 8 0.8549 [0.7914,0.9614] faster 8/8
m3/m1    discard overall  n=24 0.9779 [0.9549,0.9934] faster 21/24
m3/m1    discard 4k       n= 8 0.9270 [0.8989,0.9758] faster 8/8
m3/m1    write   overall  n=24 0.9851 [0.9712,0.9952] faster 19/24
m3/m1    write   4k       n= 8 0.9507 [0.9133,0.9819] faster 7/8
m3/m2    discard overall  n=24 0.9815 [0.9552,0.9930] faster 20/24
m3/m2    discard 4k       n= 8 0.8860 [0.8237,0.9871] faster 7/8
m3/m2    write   overall  n=24 0.9794 [0.9569,0.9877] faster 20/24
m3/m2    write   4k       n= 8 0.9131 [0.8427,0.9569] faster 8/8

main.*.json: main e6d810c (base) against round 12 as built (r12), all four classes. Summary: python3 fact.py . main r12/base
r12/base discard overall  n=32 0.9942 [0.9814,1.0022] faster 20/32
r12/base discard sd       n= 8 1.0046 [1.0020,1.0117] faster 1/8
r12/base discard 720p     n= 8 0.9994 [0.9906,1.0139] faster 4/8
r12/base discard 1080p    n= 8 0.9772 [0.9355,0.9960] faster 8/8
r12/base discard 4k       n= 8 0.8791 [0.8095,0.9861] faster 7/8
r12/base write   overall  n=32 1.0016 [0.9889,1.0059] faster 15/32
r12/base write   sd       n= 8 1.0026 [1.0012,1.0121] faster 1/8
r12/base write   720p     n= 8 1.0105 [1.0056,1.0228] faster 1/8
r12/base write   1080p    n= 8 0.9958 [0.9694,1.0108] faster 6/8
r12/base write   4k       n= 8 0.9305 [0.8555,0.9792] faster 7/8
