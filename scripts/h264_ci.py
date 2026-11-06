"""Run the registered H.264 smoke selection with exact result membership."""
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import h264_conformance


SMOKE = (
    'BA1_Sony_D', 'CABACI3_Sony_B', 'cavlc_mot_frm0_full_B', 'HCHP1_HHI_B',
    'HPCV_BRCM_A', 'alphaconformanceG', 'CVPCMNL1_SVA_C', 'MR1_BT_A',
)


def validate_selection(names, manifest):
    admitted = [row['name'] for row in manifest['streams'] if row['in_scope']]
    if len(names) != len(SMOKE) or set(names) != set(SMOKE):
        raise ValueError('smoke selection must contain exactly the eight approved names')
    if any(admitted.count(name) != 1 for name in SMOKE):
        raise ValueError('smoke selection is absent or duplicated in the admitted manifest')


def validate_report(report, mode):
    rows = report['results']
    names = [row['name'] for row in rows]
    if len(names) != len(SMOKE) or set(names) != set(SMOKE):
        raise ValueError('smoke results must contain exactly the eight approved names')
    if (report['streams'] != 8 or report['passed'] != 8 or report['failed'] != 0
            or report['threads'] != '1,2,8' or report['mode'] != mode
            or any(row['passed'] is not True or set(row['bars']) != {'C1', 'C5'}
                   or any(not value.startswith('pass ') for value in row['bars'].values())
                   for row in rows)):
        raise ValueError('smoke results require eight passing C1/C5 results at threads 1,2,8')


def main():
    args = sys.argv[1:]
    start = args.index('--only') + 1
    end = next((i for i in range(start, len(args)) if args[i].startswith('--')), len(args))
    manifest = json.loads(pathlib.Path(h264_conformance.MANIFEST).read_text())
    validate_selection(args[start:end], manifest)
    if '--manifest' in args:
        raise ValueError('smoke uses the registered manifest only')
    download = '--download-only' in args
    report = None if download else pathlib.Path(args[args.index('--report') + 1])
    if report is not None:
        report.unlink(missing_ok=True)
    try:
        h264_conformance.main()
    except SystemExit as error:
        if error.code != 0:
            raise
    if report is not None:
        mode = args[args.index('--mode') + 1] if '--mode' in args else 'fast'
        validate_report(json.loads(report.read_text()), mode)
        print('H.264 smoke: exactly 8 of 8 streams passed')


if __name__ == '__main__':
    main()
