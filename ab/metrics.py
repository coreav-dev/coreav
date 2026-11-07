import math
import numpy as np
from scipy.interpolate import PchipInterpolator

PLANES = ('y', 'u', 'v')
QPS = (22, 28, 34, 40)


def scores(original, reconstructed, width, height, frames=60):
    size = width * height * 3 // 2
    a = np.frombuffer(original, dtype=np.uint8)
    b = np.frombuffer(reconstructed, dtype=np.uint8)
    if a.size != frames * size or b.size != a.size:
        raise ValueError('incomplete visible planes')
    a, b = a.reshape(frames, size), b.reshape(frames, size)
    start, result = 0, {}
    for plane, samples in zip(PLANES, (width * height, width * height // 4, width * height // 4)):
        d = a[:, start:start + samples].astype(np.int32) - b[:, start:start + samples].astype(np.int32)
        sse = int(np.sum(d * d, dtype=np.int64))
        count = frames * samples
        result[plane] = dict(sse=sse, samples=count, psnr=10 * math.log10(255 ** 2 * count / sse) if sse else math.inf)
        start += samples
    return result


def bd_rate(candidate, baseline):
    def curve(points):
        if len(points) != 4:
            raise ValueError('four complete curve points required')
        points = sorted(points)
        x = np.array([p[0] for p in points], dtype=float)
        rates = np.array([p[1] for p in points], dtype=float)
        if not np.all(np.isfinite(x)) or not np.all(np.isfinite(rates)) or not np.all(rates > 0) or not np.all(np.diff(x) > 0):
            raise ValueError('undefined or duplicate quality support')
        return x, PchipInterpolator(x, np.log(rates), extrapolate=False)
    a, ac = curve(candidate)
    b, bc = curve(baseline)
    low, high = max(a[0], b[0]), min(a[-1], b[-1])
    if high <= low:
        raise ValueError('no common quality support')
    return float(100 * math.expm1((ac.integrate(low, high) - bc.integrate(low, high)) / (high - low)))


def compare(cells):
    expected = {(arm, qp) for arm in ('main', 'cand') for qp in QPS}
    indexed = {(cell['arm'], cell['qp']): cell for cell in cells}
    if len(indexed) != len(cells) or set(indexed) != expected:
        raise ValueError('incomplete or duplicate curve inventory')
    result = {}
    for metric in (*PLANES, 'weighted_psnr'):
        curves = []
        for arm in ('cand', 'main'):
            points = []
            for qp in QPS:
                cell = indexed[arm, qp]
                psnr = cell['scores'][metric]['psnr'] if metric != 'weighted_psnr' else sum(cell['scores'][p]['psnr'] * weight for p, weight in zip(PLANES, (6, 1, 1))) / 8
                points.append((psnr, cell['bytes'] * 8 / 2))
            curves.append(points)
        result[metric] = bd_rate(*curves)
    result['weighted_planes'] = (6 * result['y'] + result['u'] + result['v']) / 8
    result['plane_cap_pass'] = all(result[p] <= 0.5 for p in PLANES)
    return result
