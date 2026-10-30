#!/usr/bin/env python3
"""Run a campaign's shards on this machine (docs/design/fuzz-campaign.md).

  local_fuzz.py --plan DIR/plan.json --out RESULTS.json [--only NAME,NAME]
                [--jobs N] [--deadline SECONDS]

The same shards the cloud launcher runs in containers, run here as
processes: one libFuzzer per core, each with its own seed, its own corpus
directory and the plan's options, writing the result schema
`campaign.py report` reads. A campaign that fits one machine needs no
cloud and costs nothing; the release gate's 126 shards still want
the cloud launcher's containers.

A CPU-hour here is what it is there: one process with a core to itself
for an hour. Every shard is billed by the CPU time the kernel charged it
(`ru_utime + ru_stime` of the finished child), not by the clock on the
wall, so a shard that waited for a CPU while other work ran is not
counted for the wait. That does not cover the two hardware threads of
one core: the kernel charges a thread its full time while it shares the
core with a busy sibling at about half speed. So the launcher runs at
most one shard per **physical** core, its default, and refuses `--jobs`
above that; the scheduler then gives each shard a core of its own, as
long as nothing else busy runs on the machine during the campaign.
"""
import argparse
import base64
import json
import os
import pathlib
import re
import shutil
import signal
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

STAT = re.compile(r'^stat::(\w+):\s+(\d+)')
COV = re.compile(r'cov: (\d+) ft: (\d+) corp: (\d+)')
ARTIFACT_LIMIT = 1 << 20


def run_billed(argv, timeout, work, env=None):
    """Runs `argv`, and returns its exit status and the CPU time the
    kernel charged it. A child that outlives `timeout` — a hang, which is
    a finding — is killed and reported as one, never as a pass."""
    with open(work / 'stderr.log', 'wb') as log:
        child = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=log, env=env)
        deadline = time.monotonic() + timeout
        while True:
            pid, status, usage = os.wait4(child.pid, os.WNOHANG)
            if pid:
                break
            if time.monotonic() > deadline:
                # Not `child.kill()`: `Popen.send_signal` polls first and
                # would reap the child itself, leaving nothing for
                # `wait4` to account for.
                os.kill(child.pid, signal.SIGKILL)
                pid, status, usage = os.wait4(child.pid, 0)
                child.returncode = -signal.SIGKILL
                return -signal.SIGKILL, usage.ru_utime + usage.ru_stime
            time.sleep(0.2)
    code = os.WEXITSTATUS(status) if os.WIFEXITED(status) else -os.WTERMSIG(status)
    # The child is reaped; tell `Popen` so it does not wait for it again.
    child.returncode = code
    return code, usage.ru_utime + usage.ru_stime


def physical_cores():
    """The cores this process may run on, not the threads: two shards on
    the two threads of one core each run at about half speed. A CPU set
    (`taskset`, a container's cpuset) may hold both threads of one core,
    so the cores are counted from the allowed CPUs' topology, not from
    the machine's."""
    allowed = sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') \
        else list(range(os.cpu_count() or 2))
    cores = set()
    try:
        for cpu in allowed:
            topology = pathlib.Path(f'/sys/devices/system/cpu/cpu{cpu}/topology')
            # The CPUs sharing this one's core (`core_cpus_list`, older
            # kernels `thread_siblings_list`): one entry per core.
            for name in ('core_cpus_list', 'thread_siblings_list'):
                path = topology / name
                if path.exists():
                    cores.add(path.read_text().strip())
                    break
            else:
                cores = set()
                break
    except OSError:
        cores = set()
    if cores:
        return len(cores)
    # Without a topology to read, the threads this process may use are
    # all there is to count by (more than its cores, on a machine with two
    # threads a core).
    return max(1, len(allowed))


def shard(plan_dir, target, entry, options, work_root, asan=''):
    """One libFuzzer process: its own corpus, seed and artifacts."""
    name, number, seed, seconds = target['name'], entry['shard'], entry['seed'], entry['seconds']
    work = work_root / f'{name}.{number}'
    corpus, artifacts = work / 'corpus', work / 'artifacts'
    shutil.rmtree(work, ignore_errors=True)
    corpus.mkdir(parents=True)
    artifacts.mkdir(parents=True)
    seed_dir = plan_dir / 'seeds' / name
    seeds = set()
    if seed_dir.is_dir():
        for f in seed_dir.iterdir():
            if f.is_file():
                shutil.copy2(f, corpus / f.name)
                seeds.add(f.name)
    argv = [str(plan_dir / 'bin' / name), str(corpus), f'-max_total_time={seconds}',
            f'-seed={seed}', f'-artifact_prefix={artifacts}/', *options]
    started = time.monotonic()
    # The plan's AddressSanitizer options, as the cloud launcher sets them,
    # before any already set, so that an operator's own options win.
    own = os.environ.get('ASAN_OPTIONS')
    env = {**os.environ, 'ASAN_OPTIONS': f'{asan}:{own}' if own else asan} if asan else None
    code, cpu = run_billed(argv, seconds + 600, work, env)
    elapsed = time.monotonic() - started
    log = (work / 'stderr.log').read_text(errors='replace')
    stats = {k: int(v) for k, v in (m.groups() for m in map(STAT.match, log.splitlines()) if m)}
    last = COV.findall(log)
    found = []
    for f in sorted(artifacts.iterdir()):
        if f.name.startswith('slow-unit-'):
            continue
        data = f.read_bytes()
        found.append({'name': f.name, 'size': len(data),
                      'base64': base64.b64encode(data[:ARTIFACT_LIMIT]).decode()})
    finding = found[0]['name'].split('-')[0] if found else (f'exit-{code}' if code != 0 else None)
    added = [f for f in corpus.iterdir() if f.name not in seeds]
    result = {
        'name': name, 'shard': number, 'seed': seed, 'seconds': round(elapsed, 1),
        # What the kernel charged this shard: the hours a campaign is
        # counted in, whatever else the machine was doing.
        'cpu_seconds': round(cpu, 1),
        'planned_seconds': seconds, 'exit': code, 'completed': code == 0 and not found,
        'executions': stats.get('number_of_executed_units'),
        'exec_per_second': stats.get('average_exec_per_sec'),
        'peak_rss_mb': stats.get('peak_rss_mb'),
        'new_units': stats.get('new_units_added'),
        'cov': int(last[-1][0]) if last else None,
        'ft': int(last[-1][1]) if last else None,
        'corpus': int(last[-1][2]) if last else None,
        'finding': finding, 'artifacts': found,
        'log_tail': '\n'.join(log.splitlines()[-30:]),
        'corpus_added': len(added),
    }
    # A finding's input and the units this shard added stay next to the
    # results, for the regression seed and the next campaign.
    keep = work_root.parent / 'corpus' / name
    keep.mkdir(parents=True, exist_ok=True)
    for f in added:
        shutil.copy2(f, keep / f.name)
    for f in artifacts.iterdir():
        shutil.copy2(f, work_root.parent / 'corpus' / f'{name}.{f.name}')
    shutil.rmtree(work, ignore_errors=True)
    return result


def write_results(out_path, plan, jobs, results):
    """Writes the results so far. Through a temporary file: a run killed
    in the middle must not truncate the only record of what it did."""
    body = json.dumps({**plan.get('source', {}), 'host': 'local', 'jobs': jobs,
                       'shards': results}, indent=1) + '\n'
    temporary = out_path.with_suffix(out_path.suffix + '.part')
    temporary.write_text(body)
    os.replace(temporary, out_path)


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--plan', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--only', default='')
    p.add_argument('--jobs', type=int, default=0,
                   help="shards at once; by default the physical cores of the CPUs this run may use")
    p.add_argument('--deadline', type=float, default=0.0, help='stop starting shards after N seconds')
    args = p.parse_args()
    cores = physical_cores()
    if args.jobs < 0 or args.jobs > cores:
        raise SystemExit(f'--jobs {args.jobs} is not between 1 and the {cores} physical cores this run may use: two shards '
                         'on the threads of one core are each charged a full CPU-hour for half of one')
    args.jobs = args.jobs or cores
    plan_path = pathlib.Path(args.plan)
    plan = json.loads(plan_path.read_text())
    plan_dir = plan_path.parent
    names = set(filter(None, args.only.split(',')))
    def matches(target):
        # `crate__target` as the plan names it, `crate/target` as
        # `campaign.py --targets` takes it, or the bare target name.
        crate, _, bare = target['name'].partition('__')
        return {target['name'], f'{crate}/{bare}', bare} & names
    # A bare name that two targets answer to is refused, as `--targets`
    # refuses it: a run must be of the targets it names.
    for name in names:
        hits = [t['name'] for t in plan['targets'] if name in matches(t)]
        if len(hits) > 1:
            raise SystemExit(f'--only {name} names more than one target: {", ".join(hits)}')
    jobs = [(t, s) for t in plan['targets'] if not names or matches(t) for s in t['shards']]
    if names and not jobs:
        raise SystemExit(f'no target of this plan is named by --only: {args.only}')
    out_path = pathlib.Path(args.out)
    work_root = out_path.parent / 'work'
    work_root.mkdir(parents=True, exist_ok=True)
    started, results = time.monotonic(), []
    print(f'{len(jobs)} shards of {len({t["name"] for t, _ in jobs})} targets '
          f'on {args.jobs} cores', flush=True)

    def run(job):
        target, entry = job
        if args.deadline and time.monotonic() - started > args.deadline:
            return {'name': target['name'], 'shard': entry['shard'], 'seed': entry['seed'],
                    'completed': False, 'finding': None, 'seconds': 0, 'error': 'deadline'}
        return shard(plan_dir, target, entry, plan['libfuzzer'], work_root, plan.get('asan_options', ''))

    with ThreadPoolExecutor(args.jobs) as pool:
        futures = {pool.submit(run, job): job for job in jobs}
        for future in as_completed(futures):
            target, entry = futures[future]
            try:
                result = future.result()
            except Exception as e:  # noqa: BLE001 - a shard that could not run is not a pass
                result = {'name': target['name'], 'shard': entry['shard'], 'seed': entry['seed'],
                          'completed': False, 'finding': None, 'seconds': 0, 'cpu_seconds': 0,
                          'error': f'{type(e).__name__}: {e}'}
            results.append(result)
            print(f'{len(results)}/{len(jobs)} {result["name"]}.{result["shard"]}: '
                  f'{result.get("executions")} runs, cov {result.get("cov")}, '
                  f'finding {result.get("finding")}', flush=True)
            write_results(out_path, plan, args.jobs, results)
    # Every planned shard has a row, as the cloud launcher gives it one:
    # a shard with no row is a shard nobody can account for.
    done = {(r.get('name'), r.get('shard')) for r in results}
    for target, entry in jobs:
        if (target['name'], entry['shard']) not in done:
            results.append({'name': target['name'], 'shard': entry['shard'], 'seed': entry['seed'],
                            'completed': False, 'finding': None, 'seconds': 0, 'cpu_seconds': 0,
                            'error': 'no result'})
    write_results(out_path, plan, args.jobs, results)
    findings = [r for r in results if r.get('finding')]
    hours = sum(r.get('cpu_seconds') or 0 for r in results if r.get('completed')) / 3600
    print(json.dumps({'shards': len(results), 'cpu_hours': round(hours, 2),
                      'findings': [f['name'] for f in findings]}, indent=1))
    return 1 if findings else 0


if __name__ == '__main__':
    raise SystemExit(main())
