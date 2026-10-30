"""The one-pass Matroska walk, bar W5 (docs/design/policy-matroska.md, "One walk"): M7's method
(spikes/policy-matroska/run.py speed) on the held-out real Matroska and
WebM files of corpus v1: one warm-up run each, then five rounds of
`coreav check --policy web-upload.json` and `ffprobe -v error
-count_packets -show_streams`, alternating order; medians per file.

    python3 -I speed.py COREAV OUT.jsonl
"""
import json, pathlib, shutil, statistics, subprocess, sys, tempfile, time
ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
import corpus  # noqa: E402
CORE = sys.argv[1]
OUT = pathlib.Path(sys.argv[2])
WORK = OUT.parent
WEB = str(ROOT / 'docs/schema/policy-examples/web-upload.json')
entries = [e for e in json.loads((ROOT / 'corpus/v1/manifest.json').read_text())['files']
           if e['ext'] in ('mkv', 'webm') and e['set'] == 'real' and e.get('holdout')]
entries.sort(key=lambda e: e['sha256'])
done = {json.loads(l)['sha256'] for l in OUT.read_text().splitlines()} if OUT.exists() else set()

def wall(argv):
    t = time.perf_counter()
    subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
    return time.perf_counter() - t

with OUT.open('a') as sink:
    for e in entries:
        if e['sha256'] in done:
            continue
        folder = pathlib.Path(tempfile.mkdtemp(prefix='dl-', dir=WORK))
        path = folder / f"{e['sha256']}.{e['ext']}"
        r = {'sha256': e['sha256'], 'ext': e['ext'], 'size': e['size']}
        try:
            if not corpus.download(e, path):
                raise RuntimeError('size or SHA-256 mismatch')
            ours, theirs = [], []
            coreav = [CORE, 'check', '--policy', WEB, str(path)]
            ffprobe = ['ffprobe', '-v', 'error', '-count_packets', '-show_streams', str(path)]
            wall(coreav); wall(ffprobe)
            for round_ in range(5):
                order = [(coreav, ours), (ffprobe, theirs)]
                if round_ % 2:
                    order.reverse()
                for argv, acc in order:
                    acc.append(wall(argv))
            r.update(coreav=ours, ffprobe=theirs, coreav_median=statistics.median(ours),
                     ffprobe_median=statistics.median(theirs))
        except Exception as err:
            r['error'] = f'{type(err).__name__}: {err}'
        finally:
            shutil.rmtree(folder, ignore_errors=True)
        sink.write(json.dumps(r) + '\n'); sink.flush()
        print(e['sha256'][:12], r.get('coreav_median'), r.get('ffprobe_median'), r.get('error', ''), flush=True)
