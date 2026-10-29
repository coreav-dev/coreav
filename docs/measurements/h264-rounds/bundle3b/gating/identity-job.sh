set -u
mkdir -p out
chmod +x decode-*
{ lscpu | head -20; nproc; sha256sum decode-*; } > out/host.txt 2>&1
cat > one.sh <<'EOF'
f=$1; set=$2
case "$f" in *.sd.h264) n=1800;; *.720p.h264) n=900;; *.1080p.h264) n=300;; *.4k.h264) n=150;; esac
h() { "$@" | sha256sum | cut -c1-16; }
r="$set $(basename $f) $n"
for b in base k pubmain port pubmain-safe port-safe; do
  r="$r $b.1=$(h ./decode-$b --frames $n $f) $b.8=$(h ./decode-$b --frames $n --threads 8 $f)"
done
echo "$r"
EOF
ls es-train/*.h264 | xargs -P 2 -I{} bash one.sh {} tr > out/identity-train.raw 2>&1
ls es-held/*.h264 | xargs -P 2 -I{} bash one.sh {} hd > out/identity-held.raw 2>&1
python3 - <<'EOF'
import collections
out = open('out/identity.txt', 'w'); c = collections.Counter()
for p in ('out/identity-train.raw', 'out/identity-held.raw'):
    for line in sorted(open(p)):
        f = line.split(); kv = dict(x.split('=') for x in f[3:])
        g = {'gating base/k': [kv['base.1'], kv['base.8'], kv['k.1'], kv['k.8']],
             'port fast': [kv['pubmain.1'], kv['pubmain.8'], kv['port.1'], kv['port.8']],
             'port safe': [kv['pubmain-safe.1'], kv['pubmain-safe.8'], kv['port-safe.1'], kv['port-safe.8']],
             'port vs gating base': [kv['base.1'], kv['port.1']]}
        res = {k: len(set(v)) == 1 for k, v in g.items()}
        for k, v in res.items(): c[(k, v)] += 1
        out.write(' '.join(f[:3]) + ' ' + ' '.join(f'{k.replace(" ", "_")}={"same" if v else "DIFFER"}' for k, v in res.items()) + ' ' + ' '.join(f[3:]) + '\n')
open('out/identity-summary.txt', 'w').write('\n'.join(f'{k} {"same" if v else "DIFFER"} {n}' for (k, v), n in sorted(c.items())) + '\n')
EOF
