"""Discover execution boundaries and require source-bound review dispositions."""
import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import shlex
import shutil
import subprocess

ROOT = Path(__file__).resolve().parent.parent
HISTORICAL_BOUNDARY = 'docs/measurements'
HISTORICAL_RECORD = 'docs/design/oracle-historical-records.json'


def historical_records(root, record):
    if record.get('schema') != 1 or record.get('boundary') != HISTORICAL_BOUNDARY:
        raise ValueError('restore the recorded historical boundary and schema')
    expected = record.get('files')
    if not isinstance(expected, dict) or not expected or not all(isinstance(key, str) for key in expected):
        raise ValueError('restore the nonempty historical path and digest record')
    boundary = root
    for component in HISTORICAL_BOUNDARY.split('/'):
        boundary /= component
        if boundary.is_symlink() or not boundary.is_dir():
            raise ValueError('historical boundary must contain only real directories')
    actual = {path.relative_to(root).as_posix(): path for path in boundary.rglob('*')
              if path.is_file() or path.is_symlink()}
    problems = []
    for path in sorted(expected.keys() | actual.keys()):
        if not path.startswith(HISTORICAL_BOUNDARY + '/') or '..' in Path(path).parts:
            problems.append('invalid historical path: ' + path)
        elif path not in expected:
            problems.append('added historical file: ' + path)
        elif path not in actual:
            problems.append('missing historical file: ' + path)
        elif actual[path].is_symlink():
            problems.append('historical file is a symbolic link: ' + path)
        elif not isinstance(expected[path], str) or not re.fullmatch('[0-9a-f]{64}', expected[path]):
            problems.append('invalid historical digest: ' + path)
        elif hashlib.sha256(actual[path].read_bytes()).hexdigest() != expected[path]:
            problems.append('edited historical file: ' + path)
    if problems:
        raise ValueError('\n'.join(problems))
    return set(expected)

ROLE = re.compile(r'(?i)(?:ffmpeg|ffprobe|\bff\b|\bprobe\b)')
FLAGS = {'-show_streams', '-show_format', '-show_packets', '-select_streams',
         '-hide_banner', '-nostdin', '-c:v', '-c:a', '-lavfi', '-filter_complex'}
PROCESS = {'run', 'Popen', 'call', 'check_call', 'check_output', 'getoutput', 'getstatusoutput'}
OTHER = {'git', 'cargo', 'rustc', 'rustup', 'python', 'python3', 'node', 'npm', 'npx',
         'bash', 'sh', 'curl', 'tar', 'gcc', 'clang', 'pkg-config', 'ast-grep', 'uname',
         'lscpu', 'perf', 'taskset', 'sudo', 'true', 'false', 'sleep', 'mediainfo'}
BOOTSTRAP = {'scripts/oracle_pin.py', 'scripts/oracle_pin_macos.py', 'scripts/test_oracle.py'}
RUNTIME = {'crates/coreav-cli/src/fallback_cmd.rs', 'crates/coreav-worker/src/main.rs',
           'crates/coreav-backend-ffmpeg/src/lib.rs'}


def frozen_files(root, paths):
    values = {}
    for rel in ('scripts/compat_sources.py', 'tools/compat-corpus/b4_round10.py',
                'scripts/vp9_pixel_round.py', 'bench/audio-decode/capacity.py',
                'scripts/h264_pgo_diagnostics.py'):
        if (root / rel).is_symlink() or not (root / rel).resolve().is_relative_to(root):
            raise ValueError('registered source roster must not be a symbolic link')
        local = {}
        for node in ast.parse((root / rel).read_text()).body:
            if not isinstance(node, ast.Assign) or len(node.targets) != 1 or not isinstance(node.targets[0], ast.Name):
                continue
            try:
                value = (tuple(local[item.id] if isinstance(item, ast.Name) else ast.literal_eval(item)
                               for item in node.value.elts) if isinstance(node.value, ast.Tuple)
                         else ast.literal_eval(node.value))
                local[node.targets[0].id] = value
            except (ValueError, TypeError, KeyError):
                pass
        values[rel] = local
    compatibility = values['scripts/compat_sources.py']
    protected = set(compatibility['FROZEN']) | set(compatibility['JUDGING'])
    protected.add(compatibility['JUDGING_REGISTRATION'])
    protected.update(values['tools/compat-corpus/b4_round10.py']['TOOL_FILES'])
    protected.update('scripts/' + name for name in values['scripts/vp9_pixel_round.py']['CAPTURE_SCRIPTS'])
    protected.update(values['bench/audio-decode/capacity.py']['SCRIPT_INPUTS'])
    protected.update('bench/audio-decode/' + name + '.py'
                     for name in ('capacity', 'run', 'receipts', 'oracle', 'agreement'))
    protected.update(values['scripts/h264_pgo_diagnostics.py']['TOOLS'])
    protected.add('scripts/h264_pgo_acquire.py')
    return {path for path in paths if any(path == name or path.startswith(name + '/') for name in protected)}


class PythonSites:
    def __init__(self, text, path):
        self.tree = ast.parse(text)
        self.path = path
        self.sites = {}
        self.process_names = set()
        self.process_modules = {'subprocess', 'os', 'asyncio'}
        self.owners = set()
        for node in ast.walk(self.tree):
            if isinstance(node, ast.ImportFrom) and node.module == 'subprocess':
                self.process_names.update(item.asname or item.name for item in node.names if item.name in PROCESS)
            if isinstance(node, ast.ImportFrom) and node.module == 'test_oracle':
                self.owners.update(item.asname or item.name for item in node.names if item.name == 'require')
            if isinstance(node, ast.Import):
                for item in node.names:
                    if item.name in ('subprocess', 'os', 'asyncio'):
                        self.process_modules.add(item.asname or item.name)
                    if item.name == 'test_oracle':
                        self.owners.add((item.asname or item.name) + '.require')
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                arguments = [*node.args.posonlyargs, *node.args.args]
                for argument, default in zip(arguments[len(arguments) - len(node.args.defaults):], node.args.defaults):
                    name = ast.unparse(default)
                    if name in self.process_names or name.split('.', 1)[0] in self.process_modules and name.rsplit('.', 1)[-1] in PROCESS:
                        self.process_names.add(argument.arg)
        for _ in range(len(list(ast.walk(self.tree)))):
            before = len(self.process_names)
            for node in ast.walk(self.tree):
                if isinstance(node, ast.Assign):
                    name = ast.unparse(node.value)
                    if name in self.process_names or name.split('.', 1)[0] in self.process_modules and name.rsplit('.', 1)[-1] in PROCESS:
                        self.process_names.update(target.id for target in node.targets if isinstance(target, ast.Name))
            if len(self.process_names) == before:
                break

    def owner(self, node):
        name = ast.unparse(node)
        return name in self.owners

    def value(self, node, state):
        key = ast.unparse(node)
        if key in state:
            return state[key]
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            if 'literal:' + node.value in state:
                return state['literal:' + node.value]
            return 'oracle' if ROLE.search(Path(node.value).name) else 'other' if Path(node.value).name in OTHER else 'unknown'
        if isinstance(node, (ast.List, ast.Tuple)):
            if not node.elts:
                return 'argv:other'
            if isinstance(node.elts[0], ast.Constant) and isinstance(node.elts[0].value, str) and node.elts[0].value.startswith('-'):
                return 'argv:other'
            first = self.value(node.elts[0], state)
            flags = {n.value for n in node.elts if isinstance(n, ast.Constant) and isinstance(n.value, str)}
            program = ast.unparse(node.elts[0])
            if first != 'verified' and flags & FLAGS and 'coreav' not in program:
                first = 'oracle'
            return 'argv:' + first
        if isinstance(node, ast.Attribute) and node.attr in ('ffmpeg', 'ffprobe'):
            return 'verified' if self.value(node.value, state) == 'pair' else 'oracle'
        if isinstance(node, ast.Call):
            if self.owner(node.func):
                return 'pair'
            name = ast.unparse(node.func)
            if name in ('str', 'Path', 'pathlib.Path', 'os.path.abspath', 'os.fspath') and node.args:
                value = self.value(node.args[0], state)
                return value if not value.startswith('argv:') else 'unknown'
            if isinstance(node.func, ast.Attribute) and node.func.attr in ('resolve', 'absolute'):
                value = self.value(node.func.value, state)
                return value if value == 'verified' else 'oracle' if ROLE.search(key) else 'unknown'
            if name == 'getattr' and len(node.args) > 1 and isinstance(node.args[1], ast.Constant):
                return self.value(ast.Attribute(value=node.args[0], attr=node.args[1].value, ctx=ast.Load()), state)
        return 'oracle' if ROLE.search(key) else 'unknown'

    def add(self, node, state, scope, kind):
        if isinstance(node, (ast.List, ast.Tuple)):
            expression = ast.unparse(node.elts[0]) if node.elts else '[]'
        else:
            expression = ast.unparse(node)
        tag = self.value(node, state).removeprefix('argv:')
        if tag == 'other':
            if ROLE.search(ast.unparse(node)):
                tag = 'oracle'
            else:
                return
        key = (node.lineno, scope, expression)
        self.sites[key] = dict(path=self.path, line=node.lineno, scope=scope, kind=kind,
                               program=expression, status='qualified' if tag == 'verified' else 'unproven',
                               role='oracle' if tag in ('oracle', 'verified') else 'opaque')

    def expression(self, node, state, scope):
        if isinstance(node, ast.Call):
            name = ast.unparse(node.func)
            leaf = name.rsplit('.', 1)[-1]
            process = (name in self.process_names or name.split('.', 1)[0] in self.process_modules
                       and (leaf in PROCESS or leaf.startswith(('exec', 'spawn', 'posix_spawn')) or leaf in ('system', 'popen', 'create_subprocess_exec',
                                                                                  'create_subprocess_shell')))
            if process:
                argument = node.args[0] if node.args else next(
                    (keyword.value for keyword in node.keywords if keyword.arg in ('args', 'cmd', None)), node.func)
                self.add(argument, state, scope, 'process')
                for keyword in node.keywords:
                    if keyword.arg == 'executable':
                        self.add(keyword.value, state, scope, 'executable-override')
            if name in ('eval', 'exec', 'compile') and node.args:
                self.add(node.args[0], {}, scope, 'generated-execution')
            if isinstance(node.func, ast.Attribute) and node.func.attr == 'update':
                for keyword in node.keywords:
                    if keyword.arg:
                        state[ast.unparse(node.func.value) + '[' + repr(keyword.arg) + ']'] = self.value(keyword.value, state)
            if name in ('os.chdir', 'os.fchdir', 'os.putenv'):
                for key in list(state):
                    if key.startswith('literal:'):
                        del state[key]
        if isinstance(node, (ast.List, ast.Tuple)) and node.elts:
            tag = self.value(node, state)
            flags = any(isinstance(item, ast.Starred) or isinstance(item, ast.Constant)
                        and isinstance(item.value, str) and item.value.startswith('-') for item in node.elts[1:])
            if tag in ('argv:oracle', 'argv:verified') and flags:
                self.add(node, state, scope, 'command')
        for child in ast.iter_child_nodes(node):
            self.expression(child, state, scope)

    def bind(self, target, value, state):
        if isinstance(target, (ast.Tuple, ast.List)) and isinstance(value, (ast.Tuple, ast.List)):
            for left, right in zip(target.elts, value.elts):
                self.bind(left, right, state)
        else:
            state[ast.unparse(target)] = self.value(value, state)
            if isinstance(value, ast.Call) and ast.unparse(value.func).endswith(('Namespace', 'SimpleNamespace')):
                for keyword in value.keywords:
                    if keyword.arg:
                        state[ast.unparse(target) + '.' + keyword.arg] = self.value(keyword.value, state)

    def block(self, statements, state, scope):
        for statement in statements:
            if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef)):
                self.block(statement.body, dict(state), scope + '.' + statement.name)
                continue
            if isinstance(statement, ast.ClassDef):
                initial = dict(state)
                setup = next((n for n in statement.body if isinstance(n, ast.FunctionDef) and n.name == 'setUpClass'), None)
                if setup:
                    prepared = self.block(setup.body, dict(initial), scope + '.' + statement.name + '.setUpClass')
                    initial.update({key.replace('cls.', 'self.', 1): value for key, value in prepared.items()
                                    if key.startswith('cls.')})
                self.block(statement.body, initial, scope + '.' + statement.name)
                continue
            if isinstance(statement, ast.If):
                self.expression(statement.test, state, scope)
                left = self.block(statement.body, dict(state), scope)
                right = self.block(statement.orelse, dict(state), scope)
                state = {key: value for key, value in left.items() if right.get(key) == value}
                continue
            if isinstance(statement, (ast.For, ast.AsyncFor, ast.While)):
                self.expression(statement.iter if hasattr(statement, 'iter') else statement.test, state, scope)
                self.block(statement.body, dict(state), scope)
                self.block(statement.orelse, dict(state), scope)
                continue
            if isinstance(statement, (ast.Try, ast.With, ast.AsyncWith)):
                bodies = [statement.body, *([handler.body for handler in statement.handlers] if isinstance(statement, ast.Try) else [])]
                branches = [self.block(body, dict(state), scope) for body in bodies]
                state = {key: value for key, value in branches[0].items() if all(b.get(key) == value for b in branches[1:])}
                if isinstance(statement, ast.Try):
                    state = self.block(statement.finalbody, state, scope)
                continue
            self.expression(statement, state, scope)
            if isinstance(statement, ast.Assign):
                for target in statement.targets:
                    self.bind(target, statement.value, state)
            elif isinstance(statement, ast.AnnAssign) and statement.value:
                self.bind(statement.target, statement.value, state)
        return state

    def collect(self):
        self.block(self.tree.body, {}, '<module>')
        for node in ast.walk(self.tree):
            if not isinstance(node, ast.Constant) or not isinstance(node.value, str) or '\n' not in node.value:
                continue
            if ROLE.search(node.value):
                self.sites[(node.lineno, 'embedded', '')] = dict(path=self.path, line=node.lineno, scope='embedded',
                    kind='embedded-source', program='<generated program requires a separate boundary proof>',
                    status='unproven', role='opaque')
        return list(self.sites.values())


def shell_sites(text, path, offset=0):
    found = []
    result = subprocess.run(['ast-grep', 'run', '--lang', 'Bash', '--kind', 'command', '--stdin', '--json=stream'],
                            input=text, capture_output=True, text=True, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError('shell syntax discovery failed')
    for row in result.stdout.splitlines():
        node = json.loads(row)
        line = node['range']['start']['line'] + offset + 1
        source = node['text']
        try:
            tokens = shlex.split(source, comments=True)
        except ValueError:
            tokens = source.split()
        if not tokens:
            continue
        first = tokens[0]
        if first in ('echo', 'printf', 'export'):
            continue
        interpreter = first in ('bash', 'sh', 'python', 'python3', 'ruby', 'node', 'exec', 'eval', 'env', 'sudo', 'taskset')
        oracle = (bool(ROLE.search(first)) and not first.endswith(('.py', '.sh', '.rs', '.json'))
                  or interpreter and bool(ROLE.search(source)))
        dynamic = first.startswith(('$', '"$')) and '=' not in first
        if oracle or dynamic:
            found.append(dict(path=path, line=line, scope='<shell>', kind='shell', program=source.strip(),
                              status='unproven', role='oracle' if oracle else 'opaque'))
    return found


def structural_sites(root, paths, language, pattern, variable, accepted, constructor=None):
    if not paths:
        return []
    command = ['ast-grep', 'run', '--lang', language, '--pattern', pattern]
    if constructor:
        rule = {'id': 'oracle-constructor', 'language': language, 'rule': {'pattern': pattern},
                'constraints': {'CALL': {'regex': constructor}}}
        command = ['ast-grep', 'scan', '--inline-rules', json.dumps(rule)]
    result = subprocess.run([*command, '--json=stream', *paths],
                            cwd=root, capture_output=True, text=True, check=False)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr)
    found = []
    for line in result.stdout.splitlines():
        item = json.loads(line)
        if accepted and not accepted(item):
            continue
        program = item['metaVariables']['single'][variable]['text']
        name = program.strip('"\' ')
        if name in OTHER or 'current_exe(' in program:
            continue
        qualified = bool(re.search(r'(?:test_oracle|oracle_support)::require\(\)\s*\.\s*ff(?:mpeg|probe)\(\)', program))
        found.append(dict(path=item['file'], line=item['range']['start']['line'] + 1, scope='<structural>',
                          kind=language, program=program, status='qualified' if qualified else 'unproven',
                          program_span=item['metaVariables']['single'][variable]['range']['byteOffset'],
                          role='oracle' if ROLE.search(program) else 'opaque'))
    return found


def workflow_sites(text, path):
    reader = '''require 'psych'; require 'json'
rows = []
walk = lambda do |node|
  if node.is_a?(Psych::Nodes::Mapping)
    node.children.each_slice(2) do |key, value|
      if key.is_a?(Psych::Nodes::Scalar) && key.value == 'run' && value.is_a?(Psych::Nodes::Scalar)
        rows << [value.value, value.start_line + ([4, 5].include?(value.style) ? 1 : 0)]
      end
    end
  end
  (node.respond_to?(:children) && node.children || []).each { |child| walk.call(child) }
end
walk.call(Psych.parse_stream(STDIN.read))
puts JSON.generate(rows)
'''
    result = subprocess.run(['ruby', '-e', reader], input=text, capture_output=True, text=True, check=False)
    if result.returncode:
        raise ValueError('workflow run blocks could not be parsed: ' + path)
    sites = []
    for command, offset in json.loads(result.stdout):
        command = re.sub(r'\$\{\{[^}]*\}\}', '$GITHUB_VALUE', command)
        sites.extend(shell_sites(command, path, offset))
    return sites


def delegated_python_sites(root, paths):
    modules = {}
    by_name = {}
    for path in paths:
        tree = ast.parse((root / path).read_text())
        collector = PythonSites((root / path).read_text(), path)
        functions = {}
        aliases = {}
        imports = {}
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                functions.setdefault(node.name, []).append(node)
            elif isinstance(node, ast.Assign) and isinstance(node.value, (ast.Name, ast.Attribute)):
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        aliases[target.id] = node.value
            elif isinstance(node, ast.Import):
                for item in node.names:
                    imports[item.asname or item.name.split('.')[0]] = (item.name, None)
            elif isinstance(node, ast.ImportFrom) and node.module:
                for item in node.names:
                    imports[item.asname or item.name] = (node.module, item.name)
        modules[path] = (tree, collector, functions, aliases, imports)
        by_name.setdefault(Path(path).stem, set()).add(path)

    def resolve(path, function, seen):
        name = ast.unparse(function)
        if name in seen:
            return set()
        seen = {*seen, name}
        _, _, functions, aliases, imports = modules[path]
        if isinstance(function, ast.Name):
            if function.id in aliases:
                return resolve(path, aliases[function.id], seen)
            if function.id in functions:
                return {(path, function.id)}
            imported = imports.get(function.id)
            if not imported or imported[1] is None:
                return set()
            module, symbol = imported
        elif isinstance(function, ast.Attribute):
            if isinstance(function.value, ast.Name) and function.value.id in ('self', 'cls'):
                return {(path, function.attr)} if function.attr in functions else set()
            base = name.split('.')[0]
            imported = imports.get(base)
            if not imported:
                return set()
            module, _ = imported
            symbol = function.attr
        else:
            return set()
        candidates = by_name.get(module.rsplit('.', 1)[-1], set())
        return {(candidate, symbol) for candidate in candidates if symbol in modules[candidate][2]}

    edges = {}
    relays = set()
    for path, (_, collector, functions, _, _) in modules.items():
        for name, definitions in functions.items():
            identity = (path, name)
            edges[identity] = set()
            for definition in definitions:
                for node in ast.walk(definition):
                    if not isinstance(node, ast.Call):
                        continue
                    call = ast.unparse(node.func)
                    leaf = call.rsplit('.', 1)[-1]
                    if call in collector.process_names or call.split('.', 1)[0] in collector.process_modules and (
                            leaf in PROCESS or leaf.startswith(('exec', 'spawn', 'posix_spawn')) or
                            leaf in ('system', 'popen', 'create_subprocess_exec', 'create_subprocess_shell')):
                        relays.add(identity)
                    edges[identity].update(resolve(path, node.func, set()))
    while True:
        reached = {identity for identity, children in edges.items() if children & relays}
        if reached <= relays:
            break
        relays.update(reached)
    found = []
    for path, (tree, _, _, _, _) in modules.items():
        for node in ast.walk(tree):
            reached = resolve(path, node.func, set()) & relays if isinstance(node, ast.Call) else set()
            if reached:
                pending = list(reached)
                closure = set()
                while pending:
                    identity = pending.pop()
                    if identity not in closure:
                        closure.add(identity)
                        pending.extend(edges.get(identity, ()))
                found.append(dict(path=path, line=node.lineno, scope='<delegated>', kind='delegated',
                                  program=ast.unparse(node.func), status='unclassified',
                                  relay=[list(item) for item in sorted(closure)],
                                  requires=sorted({item[0] for item in closure}),
                                  role='oracle' if ROLE.search(ast.unparse(node)) else 'opaque'))
    return found


def inventory(root):
    paths = sorted(set(filter(None, subprocess.check_output(
        ['git', '-C', str(root), 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], text=True).split('\0'))))
    frozen = frozen_files(root, paths)
    if ((root / HISTORICAL_RECORD).is_symlink() or
            not (root / HISTORICAL_RECORD).resolve().is_relative_to(root)):
        raise ValueError('historical source record must not be a symbolic link')
    historical = historical_records(root, json.loads((root / HISTORICAL_RECORD).read_text()))
    found = []
    python_sources = set()
    for path in paths:
        if path in historical:
            continue
        if path == 'scripts/oracle_invocation_check.py' or path == 'scripts/tests/test_oracle_invocation_check.py':
            continue
        file = root / path
        if file.is_symlink() or not file.resolve().is_relative_to(root):
            continue
        with file.open('rb') as source:
            shebang = source.read(128).split(b'\n', 1)[0]
        if file.suffix == '.py':
            python_sources.add(path)
            found.extend(PythonSites(file.read_text(), path).collect())
        elif file.suffix in ('.sh', '.bash'):
            found.extend(shell_sites(file.read_text(), path))
        elif file.suffix in ('.yml', '.yaml'):
            found.extend(workflow_sites(file.read_text(), path))
        elif shebang.startswith(b'#!') and b'python' in shebang:
            python_sources.add(path)
            found.extend(PythonSites(file.read_text(), path).collect())
        elif shebang.startswith(b'#!') and re.search(rb'\b(?:ba|da|k|z)?sh\b', shebang):
            found.extend(shell_sites(file.read_text(), path))
    python = [path for path in sorted(python_sources) if path not in historical
              and path not in BOOTSTRAP and path not in ('scripts/oracle_invocation_check.py',
                                                        'scripts/tests/test_oracle_invocation_check.py')]
    found.extend(delegated_python_sites(root, python))
    rust = [path for path in paths if path.endswith('.rs') and not (root / path).is_symlink()
            and (root / path).resolve().is_relative_to(root) and path not in historical]
    aliases = {'Command'}
    for path in rust:
        aliases.update(re.findall(r'\bCommand\s+as\s+(\w+)', (root / path).read_text()))
    if rust and not shutil.which('ast-grep'):
        raise RuntimeError('ast-grep is required: Rust boundaries must not be silently omitted')
    constructor = r'(^|::)(' + '|'.join(re.escape(alias) for alias in sorted(aliases)) + r')::new$'
    found.extend(structural_sites(root, rust, 'Rust', '$CALL($PROGRAM)', 'PROGRAM', None, constructor))
    for method in ('spawn', 'exec', 'output', 'status'):
        found.extend(structural_sites(root, rust, 'Rust', '$PROGRAM.' + method + '()', 'PROGRAM', None))
    for path in rust:
        source = (root / path).read_text()
        if path.startswith('tools/') and '/src/' in path and 'test-support/oracle.rs' in source:
            found.append(dict(path=path, line=source[:source.index('test-support/oracle.rs')].count('\n') + 1,
                              scope='<adapter>', kind='deployment', program='<test adapter in a deployed tool>',
                              status='unproven', role='oracle'))
    javascript_aliases = set()
    for path in paths:
        if (path.endswith(('.js', '.mjs', '.cjs', '.ts', '.tsx')) and not (root / path).is_symlink()
                and (root / path).resolve().is_relative_to(root) and path not in historical):
            source = (root / path).read_text()
            javascript_aliases.update(re.findall(r'\b(?:spawnSync|spawn|execFileSync|execFile|execSync|exec)\s+as\s+(\w+)', source))
            javascript_aliases.update(re.findall(r'\b(\w+)\s*=\s*(?:\w+\.)?(?:spawnSync|spawn|execFileSync|execFile|execSync|exec)\b', source))
    javascript_functions = r'(?:^|\.)(?:spawn|spawnSync|execFile|execFileSync|execSync|exec|' + '|'.join(re.escape(name) for name in sorted(javascript_aliases)) + r')$'
    for language, suffixes in (('JavaScript', ('.js', '.mjs', '.cjs')), ('TypeScript', ('.ts',)), ('Tsx', ('.tsx',))):
        selected = [path for path in paths if path.endswith(suffixes) and not (root / path).is_symlink()
                    and (root / path).resolve().is_relative_to(root) and path not in historical]
        found.extend(structural_sites(root, selected, language, '$FUNCTION($PROGRAM, $$$ARGS)', 'PROGRAM',
                     lambda item: bool(re.search(javascript_functions,
                                                item['metaVariables']['single']['FUNCTION']['text']))))
    for site in found:
        site['owner_path_hint'] = site['status'] == 'qualified'
        site['status'] = 'unclassified'
        if site['path'] in frozen:
            site['status'] = 'frozen'
        elif site['path'] in BOOTSTRAP:
            site['status'] = 'bootstrap'
        elif site['path'] in RUNTIME:
            site['status'] = 'runtime-boundary'
    return sorted(found, key=lambda site: (site['path'], site['line'], site['kind'], site['program']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--json', type=Path)
    args = parser.parse_args()
    sites = inventory(args.root.resolve())
    counts = Counter(site['status'] for site in sites)
    unproven = [site for site in sites if site['status'] == 'unclassified']
    report = dict(counts=dict(counts), unproven_files=len({site['path'] for site in unproven}), sites=sites,
                  proof='ownership hints are not verdicts; unclassified boundaries fail closed')
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: value for key, value in report.items() if key != 'sites'}, sort_keys=True))
    return int(bool(unproven))


if __name__ == '__main__':
    raise SystemExit(main())
