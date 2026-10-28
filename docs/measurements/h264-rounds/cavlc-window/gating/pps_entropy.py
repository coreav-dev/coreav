"""python3 -I pps_entropy.py DIR: entropy_coding_mode_flag of the first PPS (7.3.2.2) of each Annex B stream in DIR; prints the CAVLC ones (flag 0)."""
import os, sys
def ue(bits, i):
    z = 0
    while bits[i] == '0': z += 1; i += 1
    return int(bits[i:i+z+1], 2) - 1, i + z + 1
for f in sorted(os.listdir(sys.argv[1])):
    b = open(os.path.join(sys.argv[1], f), 'rb').read(1 << 20)
    p = 0; flag = None
    while True:
        p = b.find(b'\x00\x00\x01', p)
        if p < 0 or p > len(b) - 12: break
        if b[p+3] & 31 == 8:
            bits = ''.join(f'{x:08b}' for x in b[p+4:p+16]); _, i = ue(bits, 0); _, i = ue(bits, i); flag = int(bits[i]); break
        p += 3
    print(f, 'CAVLC' if flag == 0 else 'CABAC' if flag == 1 else '?')
