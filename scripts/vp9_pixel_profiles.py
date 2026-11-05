"""Exclusive function-owner attribution for the VP9 pixel round."""
import collections
import gzip
import re

CATEGORIES = ('entropy_coefficients', 'inverse_transform', 'intra_prediction', 'interpolation',
              'inter_other', 'loop_filter', 'reference_packing', 'frame_management',
              'shown_picture_copy', 'unresolved')
HEADER = re.compile(r'^\S+\s+\d+/\d+\s+[\d.]+:\s+(\d+)\s+cycles:u:\s*$')
FRAME = re.compile(r'^\s*[0-9a-f]+\s+(.+)$')
LOCATION = re.compile(r'^\s+.*:\d+(?:\s+\([^)]*\))*\s*$')


def lost(record, script, flat):
    values = re.findall(rb'^# Total Lost Samples:\s*([0-9,]+)\s*$', flat, re.M)
    if len(values) != 1:
        raise ValueError('missing unique flat-report loss counter')
    count = int(values[0].replace(b',', b''))
    for data in (record, script):
        for line in data.splitlines():
            if not line.strip():
                continue
            if re.fullmatch(rb'\[ perf record: (?:Woken up [0-9]+ times to write data|'
                            rb'Captured and wrote [0-9.]+ MB .+ \([0-9]+ samples\)) \]', line):
                continue
            if line in (b'Warning:', b'Check IO/CPU overload!') and count:
                continue
            if re.fullmatch(rb'Processed [0-9]+ events and lost [0-9]+ chunks!', line) and count:
                continue
            raise ValueError('unexpected profiler diagnostic')
    return count


def samples(path, native):
    period, frames = None, []
    with gzip.open(path, 'rt') as stream:
        for line in stream:
            if LOCATION.match(line):
                if not native:
                    raise ValueError('foreign source location in symbol-only evidence')
                continue
            header = HEADER.match(line)
            if header or not line.strip():
                if period is not None:
                    yield period, frames
                period = int(header[1]) if header else None
                frames = []
                continue
            frame = FRAME.match(line)
            if frame and period is not None:
                symbol = frame[1].rsplit(' (', 1)[0]
                frames.append(re.sub(r'\[[0-9a-f]{8,}\]', '', symbol))
    if period is not None:
        yield period, frames


def native_category(frames):
    for symbol in frames:
        owner = []
        depth = 0
        for character in symbol.lstrip('<'):
            if character == '<':
                depth += 1
            elif character == '>' and depth:
                depth -= 1
            elif not depth:
                owner.append(character)
        owner = ''.join(owner)
        if owner.startswith('coreav_kernels_vp9::simd::'):
            return 'interpolation'
        if owner.startswith('coreav_kernels_vp9::inter::'):
            return 'inter_other' if '::new' in owner else 'interpolation'
        if owner.startswith('coreav_kernels_vp9::transform::'):
            return 'inverse_transform'
        if owner.startswith('coreav_kernels_vp9::intra::'):
            return 'intra_prediction'
        if owner.startswith(('coreav_kernels_vp9::filter::', 'coreav_vp9::loop_filter::')):
            return 'loop_filter'
        if owner.startswith('coreav_vp9::picture::'):
            if '::Picture::copy' in owner or '::Picture>::copy' in owner:
                return 'shown_picture_copy'
            if '::Planes' in owner and ('::picture' in owner.split('::Planes', 1)[1]):
                return 'reference_packing'
            return 'frame_management'
        if owner.startswith('coreav_vp9::mv::'):
            return 'inter_other'
        if owner.startswith(tuple('coreav_vp9::' + name + '::'
                                  for name in ('bool', 'bits', 'compressed', 'header', 'probs', 'adapt'))):
            return 'entropy_coefficients'
        if owner.startswith('coreav_vp9::tile::'):
            if owner.startswith(('coreav_vp9::tile::get::', 'coreav_vp9::tile::plane_block_size',
                                 'coreav_vp9::tile::uv_tx_size')):
                continue
            if 'reference_prediction' in owner or 'inter_prediction' in owner:
                return 'inter_other'
            if '::predict' in owner:
                return 'intra_prediction'
            if '::reconstruct' in owner:
                return 'inverse_transform'
            return 'entropy_coefficients'
        if owner.startswith(('coreav_vp9::frame_decoder::', 'coreav_backend_vp9::', 'coreav_decode::')):
            return 'frame_management'
    return 'unresolved'


def ffmpeg_category(frames):
    for symbol in frames:
        name = symbol.lower()
        if 'vp9' in name and ('loop_filter' in name or 'loopfilter' in name):
            return 'loop_filter'
        if any(part in name for part in ('decode_coeff', 'rac_get', 'read_coef', 'decode_prob',
                                         'decode_mv', 'decode_mode', 'decode_seg', 'decode_frame_header')):
            return 'entropy_coefficients'
        if 'vp9' in name and any(part in name for part in ('idct', 'iadst', 'iwht', 'itxfm')):
            return 'inverse_transform'
        if 'vp9' in name and any(part in name for part in ('ipred', 'intra_pred', 'intra_recon')):
            return 'intra_prediction'
        if (('vp9' in name and any(part in name for part in ('8tap', 'bilin', 'put_', 'avg_')))
                or re.search(r'ff_vp9_(?:put|avg)\d', name)
                or any(part in name for part in ('put_pixels', 'avg_pixels'))):
            return 'interpolation'
        if any(part in name for part in ('mc_luma', 'mc_chroma', 'vp9_mc', 'inter_pred',
                                         'find_ref_mv', 'find_mv_ref', 'fill_mv', 'scale_mv',
                                         'scaled_mv', 'splat_mv', 'ff_emu_edge')):
            return 'inter_other'
        if any(part in name for part in ('av_frame_copy_props', 'av_frame_ref', 'av_frame_unref',
                                         'av_frame_move_ref', 'av_frame_alloc', 'av_frame_free',
                                         'av_buffer_', 'ff_get_buffer', 'ff_thread_get_buffer',
                                         'ff_progress_frame', 'update_ref', 'vp9_free', 'vp9_alloc')):
            return 'frame_management'
        if name in ('av_frame_copy', 'av_image_copy', 'av_image_copy_plane'):
            return 'shown_picture_copy'
        if any(part in name for part in ('ff_vp9_decode_block', 'decode_sb')):
            return 'entropy_coefficients'
    return 'unresolved'


def analyse(path, native, user_cpu):
    weights, counts = collections.Counter(), collections.Counter()
    leaf_only = missing_callers = unknown_leaf = 0
    classify = native_category if native else ffmpeg_category
    for period, frames in samples(path, native):
        if period <= 0:
            raise ValueError('nonpositive cycle period')
        category = classify(frames)
        weights[category] += period
        counts[category] += 1
        if len(frames) == 1:
            leaf_only += period
        if not frames or frames[0].startswith('[unknown]'):
            unknown_leaf += period
        if not frames or frames[-1] in ('[unknown]', '[unknown] ([unknown])'):
            missing_callers += period
    total = sum(weights.values())
    if not total:
        raise ValueError('empty function profile')
    return dict(period=total, samples=sum(counts.values()), leaf_only_share=leaf_only / total,
                unknown_leaf_share=unknown_leaf / total,
                missing_caller_share=missing_callers / total, unresolved_share=weights['unresolved'] / total,
                components={name: dict(period=weights[name], samples=counts[name],
                                       estimated_user=user_cpu * weights[name] / total if counts[name] else None)
                            for name in CATEGORIES})
