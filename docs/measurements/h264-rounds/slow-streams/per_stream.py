"""per_stream.py RES CORE_BIN: each training stream's excess over FFmpeg by stage, and the functions behind it on both
sides, from a PMU profile's output RES (the layout of ../profile-acf3ee2f-pmu/, whose stages_pmu.py this runs, its
tables to stderr) and the CoreAV binary profiled. A stage's cycles are a share of the same stream's FFmpeg cycles in
the same record; instructions are per macroblock. Prints one line a stream and writes RES/per_stream.json."""
import contextlib, json, os, runpy, statistics, sys

res, core_bin = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
script = os.path.join(here, '..', 'profile-acf3ee2f-pmu', 'stages_pmu.py')
sys.argv = [script, res, core_bin]
with contextlib.redirect_stdout(sys.stderr):
    g = runpy.run_path(script)
st, per_item, tot, STAGES, dims = g['st'], g['per_item'], g['tot'], g['STAGES'], g['dims']
out = {}
for (s, c) in g['streams']:
    base = sum(st.get((s, c, 'ffa', 'cycles'), {}).values())
    m = lambda d, ev: statistics.mean(tot[(s, c, d)][ev]) if tot[(s, c, d)][ev] else None
    row = {'class': c, 'mbs': dims[s], 'stat_ratio': m('core', 'cycles') / m('ffb', 'cycles'),
           'ins_ratio': m('core', 'instructions') / m('ffb', 'instructions'), 'stages': {}}
    for x in STAGES:
        cc = st.get((s, c, 'core', 'cycles'), {}).get(x, 0) / base
        fc = st.get((s, c, 'ffa', 'cycles'), {}).get(x, 0) / base
        fns = {d: sorted(((k, v / base) for k, v in per_item.get((s, c, d, 'cycles', x), {}).items()),
                         key=lambda y: -y[1]) for d in ('core', 'ffa')}
        row['stages'][x] = {'core': cc, 'ffmpeg': fc, 'excess': cc - fc,
                            'ins_mb_core': st.get((s, c, 'core', 'instructions'), {}).get(x, 0) / dims[s],
                            'ins_mb_ffmpeg': st.get((s, c, 'ffa', 'instructions'), {}).get(x, 0) / dims[s],
                            'functions': fns}
    out[s] = row
json.dump(out, open(f'{res}/per_stream.json', 'w'), indent=1)
print('cycles of CoreAV over the BtbN build and instructions likewise (perf stat), then each stage\'s excess: CoreAV\'s '
      'cycles less FFmpeg\'s, as a share of FFmpeg\'s')
print('stream       class  cycles instr ' + ' '.join(f'{x:>8}' for x in STAGES + ['ent+mb']))
for s, r in sorted(out.items(), key=lambda y: -y[1]['stat_ratio']):
    e = {x: r['stages'][x]['excess'] for x in STAGES}
    e['ent+mb'] = e['entropy'] + e['mb']
    print(f"{s} {r['class']:5} {r['stat_ratio']:6.3f} {r['ins_ratio']:5.3f} "
          + ' '.join(f'{e[x]:+8.3f}' for x in STAGES + ['ent+mb']))
