"""gen.py N OUT: a no-op patch: Decoder::with_kernels takes an opaque never-taken branch into a function of N multiply steps."""
import random, sys, subprocess
n, out = int(sys.argv[1]), sys.argv[2]
src = open('crates/coreav-h264/src/decoder.rs').read()
old = "    pub fn with_kernels(kernels: K, limits: Limits) -> Decoder<K> {\n        Decoder {\n"
assert old in src
random.seed(n)
steps = ''.join(f"    x = std::hint::black_box(x.wrapping_mul(0x{random.getrandbits(64) | 1:016x}));\n" for _ in range(n))
new = ("    pub fn with_kernels(kernels: K, limits: Limits) -> Decoder<K> {\n        if std::hint::black_box(false) {\n"
       "            std::hint::black_box(placement_pad(std::hint::black_box(1)));\n        }\n        Decoder {\n")
src = src.replace(old, new) + f"\n/// Placement study: a no-op.\n#[inline(never)]\nfn placement_pad(mut x: u64) -> u64 {{\n{steps}    x\n}}\n"
open('crates/coreav-h264/src/decoder.rs', 'w').write(src)
subprocess.run(['git', 'diff', '--output', out], check=True)
subprocess.run(['git', 'checkout', '--', '.'], check=True)
