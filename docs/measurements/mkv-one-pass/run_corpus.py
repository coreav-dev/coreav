"""The one-pass Matroska walk, bars W1 to W3 on corpus v1 (docs/design/policy-matroska.md, "One
walk"): every Matroska and WebM file is downloaded from R2 (SHA-256
checked) into its own directory, run through `harness.rs` built against
fbf5824 (`h-main`) and against this change (`h-new`), and checked with
both builds of `coreav` under the three example policies.

    python3 -I run_corpus.py DIR

DIR holds target-main/release/h-main, target-new/release/h-new,
cli-main/release/coreav and cli-new/release/coreav; results go to
DIR/run/results.jsonl (corpus.jsonl here is its summary)."""
import concurrent.futures, importlib.util, json, pathlib, shutil, subprocess, sys, threading
ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'tools' / 'stage3-report'))
import corpus  # noqa: E402
import tempfile
DOWNLOADED = [0]

def download(e, work):
    folder = pathlib.Path(tempfile.mkdtemp(prefix='dl-', dir=work))
    path = folder / f"{e['sha256']}.{e['ext']}"
    if not corpus.download(e, path):
        shutil.rmtree(folder, ignore_errors=True)
        raise RuntimeError('size or SHA-256 mismatch')
    DOWNLOADED[0] += e['size']
    return path
S = pathlib.Path(sys.argv[1])  # the harness and CLI builds, and the work directory
WORK = S / 'run'
OUT = WORK / 'results.jsonl'
H = {'main': S / 'target-main/release/h-main', 'new': S / 'target-new/release/h-new'}
CLI = {'main': S / 'cli-main/release/coreav', 'new': S / 'cli-new/release/coreav'}
POL = sorted((ROOT / 'docs/schema/policy-examples').glob('*.json'))
entries = [e for e in json.loads((ROOT / 'corpus/v1/manifest.json').read_text())['files'] if e['ext'] in ('mkv', 'webm')]
done = set()
if OUT.exists():
    done = {json.loads(l)['sha256'] for l in OUT.read_text().splitlines()}
todo = [e for e in entries if e['sha256'] not in done]
print(len(entries), 'files,', len(todo), 'to do', flush=True)
lock = threading.Lock()

def one(e):
    r = {'sha256': e['sha256'], 'ext': e['ext'], 'set': e['set'], 'size': e['size'], 'holdout': e.get('holdout', False)}
    try:
        path = download(e, WORK)
    except Exception as err:
        r['error'] = f'download: {err}'
        return r
    try:
        outs = {}
        for v in ('main', 'new'):
            d = subprocess.run([str(H[v]), str(path)], capture_output=True, text=True, timeout=3600)
            outs[v] = d.stdout.replace(str(path), 'F')
            if d.returncode:
                r[f'harness_{v}_exit'] = d.returncode
        lines = {v: [l for l in outs[v].splitlines() if '\tbytes\t' not in l] for v in outs}
        r['w1_runs'] = len(lines['new'])
        r['w1_same'] = lines['main'] == lines['new']
        if not r['w1_same']:
            r['w1_diff'] = [(a, b) for a, b in zip(lines['main'], lines['new']) if a != b][:3]
        for v in outs:
            b = [l.split('\t') for l in outs[v].splitlines() if '\tbytes\t' in l]
            if b:
                r[f'bytes_{v}'] = int(b[0][3])
        r['w1_results'] = sorted({l.split('\t', 3)[3][:60] for l in lines['new']})
        same, n = True, 0
        for p in POL:
            got = {}
            for v in ('main', 'new'):
                d = subprocess.run([str(CLI[v]), 'check', '--policy', str(p), str(path)], capture_output=True, timeout=3600)
                got[v] = (d.returncode, d.stdout)
            n += 1
            if got['main'] != got['new']:
                same = False
                r.setdefault('w2_diff', []).append(p.stem)
            r.setdefault('w2_exit', {})[p.stem] = got['new'][0]
        r['w2_same'] = same
    except Exception as err:
        r['error'] = f'{type(err).__name__}: {err}'
    finally:
        shutil.rmtree(path.parent, ignore_errors=True)
    return r

with concurrent.futures.ThreadPoolExecutor(3) as pool, OUT.open('a') as sink:
    for r in pool.map(one, todo):
        with lock:
            sink.write(json.dumps(r) + '\n'); sink.flush()
            print(r['sha256'][:12], r.get('w1_same'), r.get('w2_same'), r.get('bytes_main'), r.get('bytes_new'), r.get('error', ''), flush=True)
print(json.dumps({'downloaded_bytes': DOWNLOADED[0]}), flush=True)
