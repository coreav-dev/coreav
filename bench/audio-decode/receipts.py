"""Bounded subprocess execution with closed, publishable failure receipts."""
import contextlib
import dataclasses
import json
import math
import os
import pathlib
import selectors
import signal
import subprocess
import threading
import time

BUFFER_LIMIT = 2 << 20
COUNT_LIMIT = (1 << 63) - 1
OPERATIONS = frozenset({
    'oracle.judge', 'oracle.probe', 'oracle.reader', 'oracle.model',
    'oracle.reference_decode', 'oracle.default_decode', 'oracle.coreav_decode',
    'oracle.full_decode', 'oracle.compare', 'oracle.default_compare', 'oracle.receipt',
    'capacity.stage', 'capacity.input', 'capacity.raw.base', 'capacity.raw.head',
    'capacity.cli.base', 'capacity.cli.head', 'capacity.lib.base', 'capacity.lib.head',
    'capacity.decoder.base', 'capacity.decoder.head', 'capacity.lav.library',
    'capacity.lav.decoder', 'capacity.ffmpeg',
})
TOOLS = frozenset({'harness', 'coreav', 'base_coreav', 'head_coreav', 'ffmpeg', 'ffprobe',
                   'opus_compare', 'lav', 'base_abench', 'head_abench'})
OUTCOMES = frozenset({'ok', 'disagree', 'exit', 'signal', 'timeout', 'cancelled',
                     'launch_error', 'io_error', 'protocol_error', 'internal_error', 'output_limit', 'out_of_memory'})


def receipt(operation, tool, outcome, exit_code=None, signum=None, diagnostics=None):
    return {'operation': operation if isinstance(operation, str) and operation in OPERATIONS else 'capacity.stage',
            'tool': tool if isinstance(tool, str) and tool in TOOLS else 'harness',
            'outcome': outcome if isinstance(outcome, str) and outcome in OUTCOMES else 'internal_error',
            'exit_code': exit_code if type(exit_code) is int and 0 <= exit_code <= 255 else None,
            'signal': signum if type(signum) is int and 0 < signum < 128 else None,
            'diagnostics': diagnostics or {'bytes': 0, 'lines': 0, 'dts': 0, 'unknown': 0, 'truncated': False}}


def validate(value):
    try:
        return _validate(value)
    except (ValueError, TypeError, KeyError):
        return False


def _validate(value):
    if (not isinstance(value, dict) or set(value) != {'operation', 'tool', 'outcome', 'exit_code', 'signal', 'diagnostics'}
            or value['operation'] not in OPERATIONS or value['tool'] not in TOOLS or value['outcome'] not in OUTCOMES):
        return False
    for field, upper in (('exit_code', 255), ('signal', 127)):
        number = value[field]
        if number is not None and (type(number) is not int or not 0 <= number <= upper):
            return False
    diag = value['diagnostics']
    codes_ok = value['signal'] is None
    if value['outcome'] in ('signal', 'cancelled'):
        codes_ok = (value['signal'] is None and value['outcome'] == 'cancelled'
                    or value['signal'] is not None and value['signal'] > 0)
    if value['outcome'] == 'ok':
        codes_ok = codes_ok and value['exit_code'] == 0
    if value['outcome'] == 'disagree':
        codes_ok = codes_ok and value['exit_code'] in (0, 1)
    if value['outcome'] == 'exit':
        codes_ok = codes_ok and value['exit_code'] is not None and value['exit_code'] > 0
    if value['outcome'] == 'out_of_memory':
        codes_ok = codes_ok and value['tool'] == 'opus_compare' and value['exit_code'] == 1
    return (codes_ok and isinstance(diag, dict) and set(diag) == {'bytes', 'lines', 'dts', 'unknown', 'truncated'}
            and all(type(diag[k]) is int and 0 <= diag[k] <= COUNT_LIMIT for k in ('bytes', 'lines', 'dts', 'unknown'))
            and type(diag['truncated']) is bool)


class CommandFailure(Exception):
    def __init__(self, value):
        self.receipt = value if validate(value) else receipt('capacity.stage', 'harness', 'protocol_error')
        super().__init__(self.receipt['outcome'])


def exception_receipt(error, operation='capacity.stage', tool_role='harness'):
    if isinstance(error, CommandFailure):
        return error.receipt.copy() if validate(error.receipt) else receipt(operation, tool_role, 'protocol_error')
    kind = ('cancelled' if isinstance(error, (KeyboardInterrupt, SystemExit)) else
            'io_error' if isinstance(error, OSError) else
            'protocol_error' if isinstance(error, (ValueError, KeyError, TypeError)) else 'internal_error')
    return receipt(operation, tool_role, kind)


def decode_json(data):
    def pairs(values):
        result = {}
        for key, value in values:
            if key in result:
                raise ValueError
            result[key] = value
        return result

    def constant(value):
        raise ValueError

    return json.loads(data, object_pairs_hook=pairs, parse_constant=constant)


@dataclasses.dataclass
class Result:
    outcome: str
    stdout: bytes = dataclasses.field(repr=False)
    receipt: dict
    returncode: int
    stderr: bytes = dataclasses.field(repr=False)


class Diagnostics:
    def __init__(self):
        self.value = {'bytes': 0, 'lines': 0, 'dts': 0, 'unknown': 0, 'truncated': False}
        self.pending = b''
        self.long_line = False
        self.out_of_memory = False

    def line(self):
        self.out_of_memory |= self.pending.strip().lower() == b'out of memory.'
        label = 'dts' if b'non monotonically increasing dts' in self.pending.lower() else 'unknown'
        self.value['lines'] = min(COUNT_LIMIT, self.value['lines'] + 1)
        self.value[label] = min(COUNT_LIMIT, self.value[label] + 1)
        self.pending, self.long_line = b'', False

    def feed(self, block):
        self.value['bytes'] = min(COUNT_LIMIT, self.value['bytes'] + len(block))
        for part in block.splitlines(keepends=True):
            self.pending += part[:max(0, 8192 - len(self.pending))]
            if len(part) > 8192 or len(self.pending) == 8192:
                self.long_line = True
            if part.endswith((b'\n', b'\r')):
                self.line()

    def finish(self):
        if self.pending or self.long_line:
            self.line()


@contextlib.contextmanager
def cancellation(operation, tool):
    old = {}

    def stop(signum, frame):
        raise CommandFailure(receipt(operation, tool, 'cancelled', signum=signum))

    if threading.current_thread() is threading.main_thread():
        for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP, signal.SIGQUIT):
            old[signum] = signal.signal(signum, stop)
    try:
        yield
    finally:
        for signum, handler in old.items():
            signal.signal(signum, handler)


def invoke(operation, tool_role, argv, timeout, stdout_path=None):
    if (not isinstance(operation, str) or operation not in OPERATIONS or not isinstance(tool_role, str)
            or tool_role not in TOOLS or type(timeout) not in (int, float) or not math.isfinite(timeout) or timeout <= 0):
        raise CommandFailure(receipt(operation, tool_role, 'protocol_error'))
    child = None
    output = bytearray()
    errors = bytearray()
    diagnostics = Diagnostics()
    started = time.monotonic()
    try:
        with contextlib.ExitStack() as stack:
            stack.enter_context(cancellation(operation, tool_role))
            destination = stack.enter_context(open(stdout_path, 'wb')) if stdout_path is not None else None
            selector = stack.enter_context(selectors.DefaultSelector())
            try:
                child = subprocess.Popen([str(a) for a in argv], stdin=subprocess.DEVNULL,
                                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            except OSError:
                raise CommandFailure(receipt(operation, tool_role, 'launch_error')) from None
            for pipe, name in ((child.stdout, 'stdout'), (child.stderr, 'stderr')):
                stack.callback(pipe.close)
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ, name)
            while selector.get_map() or child.poll() is None:
                remaining = timeout - (time.monotonic() - started)
                if remaining <= 0:
                    raise CommandFailure(receipt(operation, tool_role, 'timeout', diagnostics=diagnostics.value.copy()))
                if child.poll() is not None:
                    with contextlib.suppress(ProcessLookupError):
                        os.killpg(child.pid, signal.SIGKILL)
                for event, _ in selector.select(min(remaining, 0.05)):
                    block = os.read(event.fd, 65536)
                    if not block:
                        selector.unregister(event.fileobj)
                        continue
                    if event.data == 'stdout':
                        if destination is not None:
                            destination.write(block)
                        elif len(output) + len(block) > BUFFER_LIMIT:
                            raise CommandFailure(receipt(operation, tool_role, 'output_limit', diagnostics=diagnostics.value.copy()))
                        else:
                            output.extend(block)
                    else:
                        diagnostics.feed(block)
                        room = max(0, BUFFER_LIMIT - len(errors))
                        errors.extend(block[:room])
                        if len(block) > room:
                            diagnostics.value['truncated'] = True
            code = child.wait()
            diagnostics.finish()
            diag = diagnostics.value.copy()
            outcome = 'signal' if code < 0 else 'exit' if code != 0 else 'ok'
            if tool_role == 'opus_compare':
                if code in (0, 1):
                    outcome = 'ok' if code == 0 else 'disagree'
                if code == 1 and diagnostics.out_of_memory:
                    outcome = 'out_of_memory'
                if diag['truncated']:
                    outcome = 'output_limit'
            elif tool_role in ('ffmpeg', 'ffprobe') and diag['bytes'] and outcome == 'ok':
                outcome = 'protocol_error'
            value = receipt(operation, tool_role, outcome, exit_code=code if code >= 0 else None,
                            signum=-code if code < 0 else None, diagnostics=diag)
            if outcome not in ('ok', 'disagree'):
                raise CommandFailure(value)
            return Result(outcome, bytes(output), value, code, bytes(errors))
    except CommandFailure:
        raise
    except BaseException as error:
        raise CommandFailure(exception_receipt(error, operation, tool_role)) from None
    finally:
        if child is not None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGKILL)
            child.wait()


def atomic_write(path, value):
    path = pathlib.Path(path)
    temporary = path.with_name(path.name + '.partial')
    with temporary.open('w') as output:
        json.dump(value, output, allow_nan=False, separators=(',', ':'))
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)
