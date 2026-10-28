"""rec2.py ES_DIR OUTDIR CORE_BIN FFMPEG_BIN [rounds]: cpu-clock samples (perf, 4 kHz, pinned to cpu 3) of CoreAV --discard and
ffmpeg -f null at one thread (with ONLY=ff, FFmpeg alone; the attribution build is named ffmpeg_g) on the 32 training streams with the class frame counts; per stream and decoder, samples
aggregated by (dso, file address) -> OUTDIR/<stream>.<dec>.<round>.json, plus the wall time of the run."""
import collections, json, os, re, subprocess, sys, tempfile, time
ES, out, core, ff = os.path.join(sys.argv[1], ''), sys.argv[2], sys.argv[3], sys.argv[4]
rounds = int(sys.argv[5]) if len(sys.argv) > 5 else 1
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
os.makedirs(out, exist_ok=True)
mm = re.compile(r'PERF_RECORD_MMAP2? \S+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x)?([0-9a-f]+)[^\]]*\]: \S+ (\S+)')
def run(tag, argv):
    data = os.path.join(tempfile.gettempdir(), f'prof.{os.getpid()}.data')
    t = time.perf_counter()
    subprocess.run(['perf', 'record', '-q', '-F', '4000', '-e', 'cpu-clock', '-o', data, '--', 'taskset', '-c', '3'] + argv,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    wall = time.perf_counter() - t
    txt = subprocess.run(['perf', 'script', '-i', data, '--show-mmap-events', '-F', 'ip,dso'], capture_output=True, text=True).stdout
    os.remove(data)
    maps, cnt = [], collections.Counter()
    for line in txt.splitlines():
        if 'PERF_RECORD_MMAP' in line:
            m = mm.search(line)
            if m:
                maps.append((int(m.group(1), 16), int(m.group(2), 16), int(m.group(4), 16), m.group(5)))
            continue
        p = line.split()
        if len(p) < 2:
            continue
        ip = int(p[0], 16); dso = p[1].strip('()')
        key = None
        for s, ln, off, path in maps:
            if s <= ip < s + ln and path.endswith(('/' + os.path.basename(core), '/ffmpeg', '/ffmpeg_g')):
                key = (os.path.basename(path), ip - s + off); break
        if key is None:
            key = (os.path.basename(dso), 0)
        cnt[key] += 1
    json.dump({'tag': tag, 'wall': wall, 'samples': [[d, a, n] for (d, a), n in cnt.items()]}, open(f'{out}/{tag}.json', 'w'))
for r in range(rounds):
    for f in sorted(os.listdir(ES)):
        s, c = f.split('.')[0], f.split('.')[2]
        n = str(FR[c])
        if os.environ.get('ONLY') != 'ff': run(f'{s}.{c}.core.{r}', [core, '--frames', n, '--discard', ES + f])
        run(f'{s}.{c}.ff.{r}', [ff, '-nostdin', '-v', 'error', '-threads', '1', '-i', ES + f, '-frames:v', n, '-fps_mode', 'passthrough', '-f', 'null', '-'])
open(f'{out}/done', 'w').write('done\n')
