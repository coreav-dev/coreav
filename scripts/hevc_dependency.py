"""Header-declared HEVC potential dependencies, not entropy-observed block references."""

import copy
import itertools


def _validate(data):
    if not isinstance(data, dict):
        raise ValueError('expected header dependency object')
    if (data.get('mode') != 'header_dependencies'
            or data.get('header_only') is not True or data.get('complete') is not True):
        raise ValueError('expected complete header_dependencies mode')
    for field in ('coded_pictures', 'reconstruction_required', 'output_pictures', 'nals'):
        if type(data.get(field)) is not int:
            raise ValueError(f'{field} must be an integer')
    count = data['coded_pictures']
    if (not 1 <= count <= 32 or not 0 <= data['reconstruction_required'] <= count
            or not 0 <= data['output_pictures'] <= count or data['nals'] < count):
        raise ValueError('invalid header dependency counts')
    pictures = data.get('pictures')
    if not isinstance(pictures, list) or len(pictures) != count:
        raise ValueError('pictures must cover every coded picture')
    for index, picture in enumerate(pictures):
        if not isinstance(picture, dict):
            raise ValueError('picture must be an object')
        if type(picture.get('index')) is not int or picture['index'] != index:
            raise ValueError('pictures must be in coded index order')
        if type(picture.get('poc')) is not int or not -(1 << 31) <= picture['poc'] < (1 << 31):
            raise ValueError('picture POC must be an integer')
        for field in ('reconstruction_required', 'wpp'):
            if type(picture.get(field)) is not bool:
                raise ValueError(f'picture {field} must be a boolean')
        if 'envelope' in picture:
            envelope = picture['envelope']
            if not isinstance(envelope, dict):
                raise ValueError('envelope must be an object')
            for field in ('coded_width', 'coded_height', 'width', 'height', 'bit_depth',
                          'ctb_log2', 'dpb_capacity', 'highest_sps_layer'):
                if type(envelope.get(field)) is not int:
                    raise ValueError('envelope fields must be integers')
            if (not 0 < envelope['width'] <= envelope['coded_width'] <= (1 << 32) - 1
                    or not 0 < envelope['height'] <= envelope['coded_height'] <= (1 << 32) - 1
                    or envelope['bit_depth'] not in (8, 10) or envelope['ctb_log2'] not in (4, 5, 6)
                    or not 1 <= envelope['dpb_capacity'] <= 16
                    or not 0 <= envelope['highest_sps_layer'] <= 6):
                raise ValueError('invalid active SPS envelope')
        types = picture.get('slice_types')
        if (not isinstance(types, list) or not types
                or any(type(value) is not int or value not in (0, 1, 2) for value in types)
                or len(set(types)) != len(types)):
            raise ValueError('slice_types must contain unique integers 0, 1 or 2')
        for field in ('rps', 'active'):
            refs = picture.get(field)
            if not isinstance(refs, list) or len(refs) > 16:
                raise ValueError(f'{field} must contain at most 16 references')
            seen = set()
            for ref in refs:
                if (not isinstance(ref, list) or len(ref) != 3
                        or type(ref[0]) is not int or type(ref[1]) is not int
                        or type(ref[2]) is not bool):
                    raise ValueError('reference must be [coded_index, poc, long_term_bool]')
                target, poc, _ = ref
                if not 0 <= target < index or target in seen:
                    raise ValueError('reference must be a unique earlier coded picture')
                earlier = pictures[target]
                if not earlier['reconstruction_required'] or earlier['poc'] != poc:
                    raise ValueError('reference must match a reconstructed picture POC')
                seen.add(target)
            if not picture['reconstruction_required'] and refs:
                raise ValueError('skipped picture must have no references')
        if any(ref not in picture['rps'] for ref in picture['active']):
            raise ValueError('active reference must be in rps with the same identity')
        if picture['reconstruction_required']:
            if any(kind < 2 for kind in types) != bool(picture['active']):
                raise ValueError('active references must match inter slice presence')
    if sum(picture['reconstruction_required'] for picture in pictures) != data['reconstruction_required']:
        raise ValueError('reconstruction_required count does not match pictures')
    if 'output_events' in data:
        events = data['output_events']
        if not isinstance(events, list) or len(events) != data['output_pictures']:
            raise ValueError('output_events must cover every output picture')
        emitted = set()
        previous_boundary = 0
        for event in events:
            if not isinstance(event, dict) or any(type(event.get(field)) is not int
                                                 for field in ('picture_index', 'poc', 'before_picture')):
                raise ValueError('output event must contain integer picture_index, poc and before_picture')
            index, boundary = event['picture_index'], event['before_picture']
            if not 0 <= index < boundary <= count or index in emitted or boundary < previous_boundary:
                raise ValueError('output events must have unique earlier pictures and ordered boundaries')
            picture = pictures[index]
            if not picture['reconstruction_required'] or picture['poc'] != event['poc']:
                raise ValueError('output event must match a reconstructed picture POC')
            emitted.add(index)
            previous_boundary = boundary


def _metrics(pictures, field):
    vertices = [picture['index'] for picture in pictures if picture['reconstruction_required']]
    refs = {index: {ref[0] for ref in pictures[index][field]} for index in vertices}
    ancestors = {}
    depths = {}
    first_dependent = {index: len(pictures) for index in vertices}
    for index in vertices:
        ancestors[index] = set(refs[index])
        for ref in refs[index]:
            ancestors[index].update(ancestors[ref])
            first_dependent[ref] = min(first_dependent[ref], index)
        depths[index] = 1 + max((depths[ref] for ref in refs[index]), default=0)

    matched = {}

    def augment(index, seen):
        for ancestor in sorted(ancestors[index]):
            if ancestor in seen:
                continue
            seen.add(ancestor)
            if ancestor not in matched or augment(matched[ancestor], seen):
                matched[ancestor] = index
                return True
        return False

    matching = sum(augment(index, set()) for index in vertices)
    participants = set()
    non_wpp_participants = set()
    for offset, a in enumerate(vertices):
        for b in vertices[offset + 1:]:
            if b < first_dependent[a]:
                participants.update((a, b))
                if not pictures[a]['wpp'] and not pictures[b]['wpp']:
                    non_wpp_participants.update((a, b))
    admission_width = max((sum(a <= b < first_dependent[a] for a in vertices)
                           for b in vertices), default=0)
    schedules = {}
    for workers in (1, 2, 4, 8):
        completed = set()
        cursor = steps = maximum = 0
        overlap = set()
        non_wpp_overlap = set()
        while cursor < len(vertices):
            jobs = []
            while cursor < len(vertices) and len(jobs) < workers:
                index = vertices[cursor]
                if not refs[index] <= completed:
                    break
                jobs.append(index)
                cursor += 1
            steps += 1
            maximum = max(maximum, len(jobs))
            if len(jobs) > 1:
                overlap.update(jobs)
            non_wpp = [index for index in jobs if not pictures[index]['wpp']]
            if len(non_wpp) > 1:
                non_wpp_overlap.update(non_wpp)
            completed.update(jobs)
        schedules[str(workers)] = {
            'steps': steps,
            'max_workers': maximum,
            'overlap_pictures': len(overlap),
            'non_wpp_overlap_pictures': len(non_wpp_overlap),
        }
    span = max(depths.values(), default=0)
    return {
        'vertices': len(vertices),
        'edges': sum(len(value) for value in refs.values()),
        'immediate_predecessor_edges': sum(index - 1 in refs[index] for index in vertices),
        'long_term_edges': sum(ref[2] for index in vertices for ref in pictures[index][field]),
        'critical_path_unit_pictures': span,
        'maximum_antichain_width': len(vertices) - matching,
        'unit_cost_work_span_bound': len(vertices) / span if span else 0.0,
        'coded_admission_width': admission_width,
        'potential_overlap_pictures': len(participants),
        'non_wpp_potential_overlap_pictures': len(non_wpp_participants),
        'unit_duration_coded_order': schedules,
    }


def _receive_barrier(pictures, field, events, *, delayed_sealing=False, output_lag3=False):
    boundaries = {event['before_picture'] for event in events}
    schedules = {}
    for workers in ((4,) if output_lag3 else (1, 2, 4, 8)):
        time = 0
        starts = {}
        finishes = {}
        pending = []
        max_pending = 0

        def fence_finish(boundary):
            return max((finish for earlier, finish in finishes.items() if earlier < boundary), default=0)

        for boundary in range(len(pictures) + 1):
            index = boundary - 1 if delayed_sealing else boundary
            if 0 <= index < len(pictures) and pictures[index]['reconstruction_required']:
                picture = pictures[index]
                time = max(time, max((finishes[ref[0]] for ref in picture[field]), default=0))
                active = [finish for finish in finishes.values() if finish > time]
                if len(active) >= workers:
                    time = min(active)
                starts[index] = time
                finishes[index] = time + 1
            if delayed_sealing and boundary < len(pictures):
                time = max(time, max((finishes[ref[0]] for ref in pictures[boundary][field]), default=0))
            if output_lag3:
                for event in events:
                    if event['before_picture'] != boundary:
                        continue
                    if len(pending) == 3:
                        time = max(time, fence_finish(pending[0]))
                        pending.pop(0)
                    pending.append(boundary)
                    max_pending = max(max_pending, len(pending))
                while pending and fence_finish(pending[0]) <= time:
                    pending.pop(0)
            elif boundary in boundaries:
                time = max(time, fence_finish(boundary))
        if output_lag3:
            time = max(time, max(finishes.values(), default=0))
            while pending:
                time = max(time, fence_finish(pending[0]))
                pending.pop(0)
        overlap = set()
        non_wpp_overlap = set()
        for a, b in itertools.combinations(starts, 2):
            if max(starts[a], starts[b]) < min(finishes[a], finishes[b]):
                overlap.update((a, b))
                if not pictures[a]['wpp'] and not pictures[b]['wpp']:
                    non_wpp_overlap.update((a, b))
        maximum = max((sum(start <= time < finishes[index] for index, start in starts.items())
                       for time in starts.values()), default=0)
        schedules[str(workers)] = {
            'steps': max(finishes.values(), default=0),
            'max_workers': maximum,
            'overlap_pictures': len(overlap),
            'non_wpp_overlap_pictures': len(non_wpp_overlap),
        }
        if output_lag3:
            schedules[str(workers)]['max_pending_outputs'] = max_pending
    return schedules


def analyze(data):
    """Validate a decoded JSON object; return replayable input and two graph models.

    RPS contains every used_by_curr_pic member. Active is the Rust active-list
    owner's union across independently signalled segments: I segments contribute
    no references and dependent segments inherit. Neither graph observes blocks.
    """
    _validate(data)
    result = {
        'graph': copy.deepcopy(data),
        'semantics': {
            'references': 'header-declared potential references, not entropy-observed block references',
            'rps': 'every used_by_curr_pic member',
            'active': 'Rust active-list owner union across independently signalled segments; '
                      'I segments have no references; dependent segments inherit',
            'vertices': 'reconstruction_required pictures only; indices retain coded order',
            'immediate_predecessor_edges': 'references to coded index minus one',
            'dag': 'optimistic dependency availability, not coded-order admission',
            'coded_admission': 'intervals [coded index, first later direct dependent); '
                               'no dependent ends at coded_pictures',
            'non_wpp': 'participants of pairs with both pictures non-WPP; optimistic frame-job '
                       'eligibility, not observed overlap',
            'unit_cost_work_span_bound': 'unit-picture work divided by unit-picture critical path; '
                                         'equal-cost DAG bound, not measured throughput',
            'unit_duration_coded_order': 'equal-cost unit-duration list scheduling at 1/2/4/8 workers; '
                                         'admit next only with all references completed and a free slot, '
                                         'then complete active jobs in one step; not measured throughput '
                                         'or a general duration-free speedup',
            'excluded': ['output', 'reorder', 'capacity', 'SEI', 'memory barriers'],
        },
        'rps': _metrics(data['pictures'], 'rps'),
        'active': _metrics(data['pictures'], 'active'),
    }
    if 'output_events' in data:
        for field in ('rps', 'active'):
            result[field]['unit_duration_receive_barrier'] = _receive_barrier(
                data['pictures'], field, data['output_events'])
            result[field]['unit_duration_delayed_sealing_receive_barrier'] = _receive_barrier(
                data['pictures'], field, data['output_events'], delayed_sealing=True)
            lagged = {'workers': 4, 'output_lag_limit': 3}
            for mode in ('picture_ready', 'delayed_sealing'):
                metrics = _receive_barrier(data['pictures'], field, data['output_events'],
                                           delayed_sealing=mode == 'delayed_sealing', output_lag3=True)['4']
                lagged[mode] = {'full_picture_references': metrics,
                                'row_references_worst_case': copy.deepcopy(metrics)}
            result[field]['unit_duration_output_lag3'] = lagged
        result['semantics']['unit_duration_receive_barrier'] = (
            'optimistic picture-ready access-unit equal-cost model at 1/2/4/8 workers; '
            'one coded picture per send and receive-until-None after every send, including skipped pictures; '
            'admit current required picture only with completed references and a free worker; '
            'after admitting i, an output event before_picture=i waits for every job with coded index<i, '
            'so current job i may overlap earlier jobs; all jobs take one time unit and EOF completes all jobs; '
            'overlap requires intersecting execution intervals; ignores existing one-picture-late job sealing, '
            'output processing costs, capacity, SEI and memory barriers; not measured throughput, '
            'a general duration-free speedup or an arbitrary-packet-grouping model')
        result['semantics']['unit_duration_delayed_sealing_receive_barrier'] = (
            'existing one-picture-late submission timing under the proposed receive barrier, '
            'equal unit durations at 1/2/4/8 workers; one coded picture per send, including skipped calls; '
            'call i first seals and submits required job i-1 subject to completed references and a free worker, '
            'then waits for references of current picture i before accepting its input, '
            'then applies output events before_picture=i by joining all submitted jobs with index<i; '
            'EOF call coded_pictures seals the final job and completes every job; '
            'RPS uses the actual Scheduler.required_references used-RPS gate; active substitutes active '
            'references and is not the existing scheduler gate; overlap requires intersecting execution intervals; '
            'no memory, SEI or CPU timing model; not measured throughput, a full implementation simulator, '
            'a general duration-free speedup or an arbitrary-packet-grouping model')
        result['semantics']['excluded_scope'] = (
            'excluded applies to DAG and unit_duration_coded_order; '
            'receive-barrier and output-lag models additionally model output visibility causal fences')
        result['semantics']['unit_duration_output_lag3'] = (
            'optimistic abstract capacity-driven producer/consumer backpressure at four workers, '
            'one coded picture per send with equal unit durations; picture_ready dispatches the current job, '
            'delayed_sealing submits the previous job and waits for current input references as above; '
            'lag counts FIFO normative output records not yet consumed: logical pending emitted frames, '
            'not an allocated-byte/capacity bound; '
            'process each call output event in order: before appending with three records held, '
            'join and consume the oldest record full all-earlier-jobs causal fence, then append; '
            'receive-until-None consumes ready front records without waiting; EOF joins all jobs and drains outputs; '
            'max_pending_outputs witnesses the limit before any fourth append; '
            'row_references_worst_case equals full_picture_references because no CTU-row or MV footprints exist; '
            'publication is not reclamation and never narrows a causal prefix or removes reference availability; '
            'assumes a producer/consumer rendezvous inside admission, not implementability under the current '
            'Decoder send/receive API or a byte-budget proof; no memory, SEI or CPU timing model; '
            'not measured throughput, a full implementation simulator or an arbitrary-packet-grouping model')
    return result
