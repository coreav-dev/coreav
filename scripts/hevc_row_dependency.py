"""Actual prediction-read traces with hypothetical equal-row-cost publication models."""

import copy

from hevc_dependency import analyze as analyze_header


def _validate(data):
    if not isinstance(data, dict):
        raise ValueError('expected row dependency object')
    if (data.get('mode') != 'row_dependencies' or data.get('complete') is not True
            or data.get('header_only') is not False):
        raise ValueError('expected complete row_dependencies capture')
    for field in ('requested_threads', 'thread_limit', 'max_workers'):
        if type(data.get(field)) is not int or data[field] != 1:
            raise ValueError('row capture must use one requested, bounded and observed worker')
    header = data.get('header_dependencies')
    analyze_header(header)
    if (type(data.get('output_pictures')) is not int
            or data['output_pictures'] != header['output_pictures']):
        raise ValueError('output_pictures must match the header output count')
    if 'output_events' not in header or any(picture['wpp'] for picture in header['pictures']):
        raise ValueError('row capture requires output events and no WPP')
    expected = [picture for picture in header['pictures'] if picture['reconstruction_required']]
    if type(data.get('decoded_pictures')) is not int or data['decoded_pictures'] != len(expected):
        raise ValueError('decoded_pictures must cover every required header picture')
    pictures = data.get('pictures')
    if not isinstance(pictures, list) or len(pictures) != len(expected):
        raise ValueError('pictures must cover every required header picture')
    earlier = {}
    for picture, identity in zip(pictures, expected):
        if not isinstance(picture, dict):
            raise ValueError('row picture must be an object')
        for field in ('index', 'poc'):
            if type(picture.get(field)) is not int or picture[field] != identity[field]:
                raise ValueError('row picture identity must match coded header order')
        for field in ('width', 'height', 'ctb_log2'):
            if type(picture.get(field)) is not int or not 1 <= picture[field] <= (1 << 32) - 1:
                raise ValueError('row geometry must contain positive u32 integers')
        if picture['ctb_log2'] not in (4, 5, 6):
            raise ValueError('ctb_log2 must be 4, 5 or 6')
        if type(picture.get('tiles')) is not bool or picture.get('wpp') is not False:
            raise ValueError('tiles must be boolean and WPP must be disabled')
        count = (picture['height'] + (1 << picture['ctb_log2']) - 1) >> picture['ctb_log2']
        rows = picture.get('rows')
        if not 1 <= count <= 2048 or not isinstance(rows, list) or len(rows) != count:
            raise ValueError('rows must cover the CTU geometry within the 2048-row cap')
        allowed = {ref[0]: ref[1] for ref in identity['rps']}
        for ordinal, row in enumerate(rows):
            if not isinstance(row, dict) or type(row.get('row')) is not int or row['row'] != ordinal:
                raise ValueError('rows must be in complete CTU row order')
            for field in ('sample_calls', 'motion_calls'):
                if type(row.get(field)) is not int or row[field] < 0:
                    raise ValueError('row call counts must be nonnegative integers')
            refs = row.get('references')
            if not isinstance(refs, list) or len(refs) > 16:
                raise ValueError('row references must contain at most 16 identities')
            seen = set()
            reads = [0, 0]
            for ref in refs:
                if (not isinstance(ref, list) or len(ref) != 4
                        or type(ref[0]) is not int or type(ref[1]) is not int):
                    raise ValueError('row reference must contain index, POC, sample row and motion row')
                index, poc, sample, motion = ref
                if (index in seen or index not in earlier or index not in allowed
                        or earlier[index]['poc'] != poc or allowed[index] != poc):
                    raise ValueError('row reference must match a unique earlier real RPS picture')
                if sample is None and motion is None:
                    raise ValueError('row reference must record an actual sample or motion read')
                for kind, maximum in enumerate((sample, motion)):
                    if maximum is not None:
                        if type(maximum) is not int or not 0 <= maximum < len(earlier[index]['rows']):
                            raise ValueError('read maximum must lie inside reference CTU rows')
                        reads[kind] += 1
                seen.add(index)
            for distinct_reads, field in zip(reads, ('sample_calls', 'motion_calls')):
                if row[field] < distinct_reads or bool(row[field]) != bool(distinct_reads):
                    raise ValueError('row call counts must agree with actual referenced reads')
            if identity['slice_types'] == [2] and refs:
                raise ValueError('I-only picture cannot contain prediction reads')
        earlier[picture['index']] = picture


def _tasks(data, filter_lag):
    pictures = {picture['index']: picture for picture in data['pictures']}
    nodes = {index: [(index, row) for row in range(1 if picture['tiles'] else len(picture['rows']))]
             for index, picture in pictures.items()}
    costs = {}
    dependencies = {}
    depths = {}
    for index, picture in pictures.items():
        header = data['header_dependencies']['pictures'][index]
        for row, node in enumerate(nodes[index]):
            costs[node] = len(picture['rows']) if picture['tiles'] else 1
            refs = {nodes[index][row - 1]} if row else set()
            if filter_lag is None or picture['tiles']:
                if not row:
                    refs.update(nodes[ref[0]][-1] for ref in header['rps'])
            else:
                for target, _, sample, motion in picture['rows'][row]['references']:
                    if pictures[target]['tiles']:
                        refs.add(nodes[target][-1])
                        continue
                    if sample is not None:
                        refs.add(nodes[target][min(sample + filter_lag, len(nodes[target]) - 1)])
                    if motion is not None:
                        refs.add(nodes[target][motion])
            dependencies[node] = refs
            depths[node] = costs[node] + max((depths[ref] for ref in refs), default=0)
    return nodes, costs, dependencies, max(depths.values(), default=0)


def _simulate(data, filter_lag, delayed_sealing):
    nodes, costs, dependencies, span = _tasks(data, filter_lag)
    header = data['header_dependencies']
    progress = {index: 0 for index in nodes}
    remaining = dict(costs)
    admitted = set()
    completed = set()
    done = set()
    pending = []
    overlap = set()
    time = maximum_workers = maximum_admitted = maximum_pending = 0

    def tick():
        nonlocal time, maximum_workers
        ready = [index for index in sorted(admitted)
                 if dependencies[nodes[index][progress[index]]] <= done]
        if not ready:
            raise ValueError('row model has no runnable admitted work')
        maximum_workers = max(maximum_workers, len(ready))
        if len(ready) > 1:
            overlap.update(ready)
        for index in ready:
            node = nodes[index][progress[index]]
            remaining[node] -= 1
            if not remaining[node]:
                done.add(node)
                progress[index] += 1
                if progress[index] == len(nodes[index]):
                    completed.add(index)
                    admitted.remove(index)
        time += 1

    def prefix_complete(boundary):
        return all(index in completed for index in nodes if index < boundary)

    for call in range(header['coded_pictures'] + 1):
        index = call - int(delayed_sealing)
        if index in nodes:
            required = {ref[0] for ref in header['pictures'][index]['rps']} if filter_lag is None else set()
            while len(admitted) == 4 or not required <= completed:
                tick()
            admitted.add(index)
            maximum_admitted = max(maximum_admitted, len(admitted))
        if delayed_sealing and filter_lag is None and call < header['coded_pictures']:
            required = {ref[0] for ref in header['pictures'][call]['rps']}
            while not required <= completed:
                tick()
        for event in header['output_events']:
            if event['before_picture'] != call:
                continue
            if len(pending) == 3:
                while not prefix_complete(pending[0]):
                    tick()
                pending.pop(0)
            pending.append(call)
            maximum_pending = max(maximum_pending, len(pending))
        while pending and prefix_complete(pending[0]):
            pending.pop(0)
    while admitted:
        tick()
    pending.clear()
    work = sum(costs.values())
    mean_rows = work / len(nodes) if nodes else 0
    return {
        'steps': time,
        'max_workers': maximum_workers,
        'serial_row_work': work,
        'critical_path_row_units': span,
        'normalized_picture_work': len(nodes),
        'normalized_picture_span': span / mean_rows if mean_rows else 0.0,
        'normalized_picture_steps': time / mean_rows if mean_rows else 0.0,
        'serial_work_over_steps': work / time if time else 0.0,
        'unit_row_work_span_bound': work / span if span else 0.0,
        'four_worker_work_span_bound': min(4.0, work / span) if span else 0.0,
        'max_in_flight_pictures': maximum_admitted,
        'max_pending_outputs': maximum_pending,
        'overlap_pictures': len(overlap),
    }


def analyze(data):
    """Validate actual single-worker read traces and replay hypothetical four-worker models."""
    _validate(data)
    models = {}
    for mode in ('picture_ready', 'delayed_sealing'):
        models[mode] = {name: _simulate(data, lag, mode == 'delayed_sealing')
                        for name, lag in (('filter_lag1', 1), ('filter_lag2', 2),
                                          ('whole_reference_control', None))}
    return {
        'graph': copy.deepcopy(data),
        'tiled_picture_indices': [picture['index'] for picture in data['pictures'] if picture['tiles']],
        'models': models,
        'semantics': {
            'capture': 'actual clipped interpolation-read maxima across all components and separate '
                       'TMVP motion-field read maxima, not header-declared potential reads',
            'cost': 'hypothetical equal CTU-row unit costs including amortized filter work; '
                    'serial rows within each picture, at most four unfinished admitted pictures and four '
                    'CPU workers; waiting pictures consume no CPU worker; unequal row counts retain unequal work',
            'filter_lag1': 'hypothetical incremental filtering, not current whole-picture filtering; '
                           'sample row q waits for reconstruction row min(q+1,last) to finish for '
                           'horizontal deblocking and the one-pixel SAO margin; motion reads wait only for their row; '
                           'the last row publishes at picture completion',
            'filter_lag2': 'separately labeled conservative sample lookahead min(q+2,last); '
                           'motion-field readiness is unchanged',
            'tiles': 'whole-picture readiness and publication fallback: a tiled picture is an atomic job '
                     'costing its row count, starts after complete header RPS references and publishes only '
                     'on completion; its row labels imply no raster-row entropy order; consumers wait for '
                     'complete tiled references',
            'admission': 'picture_ready admits the current picture; delayed_sealing submits the previous '
                         'picture at the next boundary and seals the final picture at EOF; row variants '
                         'hypothetically replace whole-reference input gates with per-row waits; '
                         'whole_reference_control retains complete used-RPS submission and current-input gates',
            'outputs': 'FIFO lag-three logical pending emitted frames; before appending a fourth record '
                       'join and consume the oldest all-rows-of-all-earlier-pictures causal fence; '
                       'receive-until-None consumes only ready front records; EOF joins all jobs and drains '
                       'records; publication is not reclamation and never removes reference availability',
            'normalization': 'divide row-unit span and modeled steps by mean rows per required picture; '
                             'normalized_picture_work is the required picture count; work/span ignores '
                             'admission and output fences; four_worker_work_span_bound is min(4,work/span), '
                             'a DAG speedup upper bound conditional on equal-row costs and hypothetical filter '
                             'publication, while serial_work_over_steps uses the modeled schedule',
            'limits': 'optimistic abstract producer/consumer rendezvous inside admission, not implementability '
                      'under the current Decoder send/receive API, an allocated-byte/capacity proof, '
                      'measured throughput or a duration-free speedup; no pipeline implementation, '
                      'memory, SEI, packet-grouping or CPU timing model',
        },
    }
