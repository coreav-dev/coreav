#!/usr/bin/env python3
"""Run one job on a short-lived Compute Engine VM with hardware performance counters, and delete the VM.

    gcp_vm_job.py --upload LOCAL_DIR --job job.sh --download RESULTS_DIR [--state STATE_DIR] [--max-hours 2]
                  [--cap-usd 2] [--provisioning spot|standard] [--threads-per-core N] [--turbo-mode ALL_CORE_MAX]
    gcp_vm_job.py --collect STATE_DIR

Creates a VM (default a SPOT c4-standard-8 in us-central1-a, Debian, hyperdisk-balanced boot disk) with
`advancedMachineFeatures.performanceMonitoringUnit` set, and `threadsPerCore` where `--threads-per-core` is given
(1 turns simultaneous multithreading off: the VM has one vCPU a physical core, and is billed for the machine's full
vCPU count), and `turboMode` where `--turbo-mode` is given (ALL_CORE_MAX holds every core at the all-core turbo
frequency instead of letting a busy core boost above it; C4 and C4N only). `--provisioning standard` makes an on-demand
VM, which is not preempted. What Compute Engine reports for the VM (machine type, zone, CPU platform, advanced machine
features, provisioning; not its name) is written to RESULTS_DIR/instance.json before the job runs. Copies LOCAL_DIR to `~/job`
on it over SSH with a key made for this run, starts `bash ~/job/JOB` there with a hard timeout and detached from the
SSH session, waits for its exit status (`~/job/job.status`), copies `~/job/out` back to RESULTS_DIR, and deletes the
VM, also when a step before fails (not when its insert is refused as a name in use). The VM also carries
`scheduling.maxRunDuration` with `instanceTerminationAction=DELETE`, so it deletes itself if this script dies first.

After a restart: the ssh key and the job's state (the VM's name, zone and project, RESULTS_DIR, the hours, the price
bound, the cap and the start time) are kept in STATE_DIR (default RESULTS_DIR.gcpjob; it must be new or empty), not in a
temporary directory, and a .gitignore in it keeps git from staging them. When this script stops before the end, for
example because the machine that runs it restarts, `--collect STATE_DIR` finishes the job as the run would have: it
waits for the job's exit status, copies `~/job/out` back, deletes the VM and writes spend.json. It keeps the run's time
limit, counted from the saved start time: past it, it deletes the VM without waiting. It is safe to run again: once the
VM is gone, it only prints the recorded outcome. The ssh key is deleted with the VM. The output counts as come back only
when its whole copy ended without an error (three attempts); otherwise the VM is deleted all the same and the run fails.
The exit status is 0 when the job's output came back, whatever the job's own exit status (in RESULTS_DIR/job.status),
and 1 when it did not. STATE_DIR names the cloud project: keep it out of published results.

Credentials: COREAV_GCP_SA_KEY (a service-account key, JSON text or a path to it) and COREAV_GCP_PROJECT (`--collect`
reads the project from STATE_DIR). The key is
never printed or written anywhere; the access token is kept in memory. Only the Compute Engine REST API (v1) and the
OAuth token endpoint are used, through the standard library; `ssh`, `scp` and `ssh-keygen` must be on PATH.

Spend: the script refuses to start unless `max-hours * usd-per-hour` is at most `cap-usd`. `usd-per-hour` is an upper
bound on the VM's hourly price (0.60 for the default c4-standard-8: its on-demand price in us-central1 plus the boot
disk, rounded up; required for any other machine), so the cap holds whatever the SPOT price is and covers a standard
VM. The bound actually used (hours the VM existed, at most `max-hours`, times `usd-per-hour`) is printed at the end
and written to RESULTS_DIR/spend.json.
"""
import argparse, base64, hashlib, json, os, shlex, subprocess, time, urllib.error, urllib.parse, urllib.request

API = 'https://compute.googleapis.com/compute/v1'
POLL = 30


# --- OAuth with a service-account key (RS256 JWT), standard library only ---------------------------------------------

def _der(b, i):
    """One DER element at b[i]: (tag, content start, content end)."""
    tag, n = b[i], b[i + 1]
    i += 2
    if n & 0x80:
        k = n & 0x7f
        n = int.from_bytes(b[i:i + k], 'big')
        i += k
    return tag, i, i + n


def _rsa_key(pem):
    """(n, d) of a PKCS#8 'BEGIN PRIVATE KEY' RSA key."""
    body = ''.join(l for l in pem.strip().splitlines() if not l.startswith('-----'))
    b = base64.b64decode(body)
    _, i, _ = _der(b, 0)                 # PrivateKeyInfo SEQUENCE
    _, s, e = _der(b, i)                 # version
    _, s, e = _der(b, e)                 # algorithm SEQUENCE
    _, s, e = _der(b, e)                 # OCTET STRING holding RSAPrivateKey
    _, i, _ = _der(b, s)                 # RSAPrivateKey SEQUENCE
    ints = []
    while len(ints) < 4:                 # version, n, e, d
        _, s, e = _der(b, i)
        ints.append(int.from_bytes(b[s:e], 'big'))
        i = e
    return ints[1], ints[3]


def _sign(n, d, msg):
    """RSASSA-PKCS1-v1_5 with SHA-256 (RFC 8017, section 8.2)."""
    k = (n.bit_length() + 7) // 8
    t = bytes.fromhex('3031300d060960864801650304020105000420') + hashlib.sha256(msg).digest()
    em = b'\x00\x01' + b'\xff' * (k - len(t) - 3) + b'\x00' + t
    return pow(int.from_bytes(em, 'big'), d, n).to_bytes(k, 'big')


def access_token(key):
    b64 = lambda x: base64.urlsafe_b64encode(x).rstrip(b'=')
    now = int(time.time())
    head = b64(json.dumps({'alg': 'RS256', 'typ': 'JWT'}).encode())
    claim = b64(json.dumps({'iss': key['client_email'], 'scope': 'https://www.googleapis.com/auth/compute',
                            'aud': key['token_uri'], 'iat': now, 'exp': now + 3600}).encode())
    n, d = _rsa_key(key['private_key'])
    jwt = head + b'.' + claim + b'.' + b64(_sign(n, d, head + b'.' + claim))
    data = urllib.parse.urlencode({'grant_type': 'urn:ietf:params:oauth:grant-type:jwt-bearer',
                                   'assertion': jwt.decode()}).encode()
    with urllib.request.urlopen(urllib.request.Request(key['token_uri'], data=data), timeout=30) as r:
        return json.load(r)['access_token']


def load_key(raw):
    """The service-account key from COREAV_GCP_SA_KEY: JSON text, or a path to a JSON file. No error message holds
    the value, whatever form it has."""
    text = raw.lstrip('\ufeff').strip()
    try:
        if not text.startswith('{') and os.path.isfile(raw):
            with open(raw, encoding='utf-8-sig') as f:
                text = f.read().strip()
        key = json.loads(text)
        if not all(k in key for k in ('client_email', 'private_key', 'token_uri')):
            raise ValueError
        return key
    except (OSError, ValueError):
        raise SystemExit('COREAV_GCP_SA_KEY is neither service-account JSON nor a path to it') from None


class Compute:
    def __init__(self, key):
        self.key = key
        self.token, self.at = None, 0

    def call(self, method, path, body=None, ok404=False):
        if time.time() - self.at > 1800:
            self.token, self.at = access_token(self.key), time.time()
        url = path if path.startswith('https://') else f'{API}/{path}'
        req = urllib.request.Request(url, method=method, data=None if body is None else json.dumps(body).encode(),
                                     headers={'Authorization': f'Bearer {self.token}',
                                              'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            if ok404 and e.code == 404:
                return None
            # The API's error message, which names the field at fault; it never holds the key or the token.
            raise SystemExit(f'{method} {path}: HTTP {e.code}: {e.read().decode(errors="replace")[:2000]}')

    def wait(self, op, limit=600):
        t = time.time()
        while op.get('status') != 'DONE':
            if time.time() - t > limit:
                raise SystemExit(f'operation {op.get("name")} not done after {limit} s')
            time.sleep(3)
            op = self.call('GET', op['selfLink'])
        if 'error' in op:
            raise SystemExit(f'operation failed: {json.dumps(op["error"])[:2000]}')
        return op


def ssh_ready(ssh, limit=300):
    t = time.time()
    while time.time() - t < limit:
        if subprocess.run(ssh + ['true'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                          timeout=60).returncode == 0:
            return
        time.sleep(5)
    raise SystemExit(f'no SSH after {limit} s')


def credentials():
    raw = os.environ.get('COREAV_GCP_SA_KEY', '')
    if not raw:
        raise SystemExit('COREAV_GCP_SA_KEY must be set')
    return load_key(raw)


def save(state_dir, s):
    part = os.path.join(state_dir, 'state.json.part')
    with open(part, 'w') as f:
        json.dump(s, f, indent=1)
    os.replace(part, os.path.join(state_dir, 'state.json'))


def ssh_opts(state_dir):
    return ['-i', os.path.join(state_dir, 'id'), '-o', 'StrictHostKeyChecking=no',
            '-o', f'UserKnownHostsFile={os.path.join(state_dir, "known")}', '-o', 'ConnectTimeout=20',
            '-o', 'ServerAliveInterval=30', '-o', 'LogLevel=ERROR']


def zone_path(s):
    return f'projects/{s["project"]}/zones/{s["zone"]}'


def fetch(gce, s, state_dir):
    """Wait for ~/job/job.status until the deadline, then copy ~/job/out to the download directory: the job's status
    and whether its output came back. Only a copy that ended without an error is recorded as come back, in the
    state, before the VM is deleted: a file of the output on disk does not show that the rest of it came too."""
    opts = ssh_opts(state_dir)
    print('waiting for the job', flush=True)
    while True:
        inst = gce.call('GET', f'{zone_path(s)}/instances/{s["name"]}', ok404=True)
        if inst is None:
            return 'the VM is gone', False
        if inst.get('status') in ('STOPPING', 'STOPPED', 'SUSPENDING', 'SUSPENDED', 'TERMINATED'):
            return f'the VM is {inst["status"]}', False
        if time.time() > s['deadline']:
            return 'the time limit passed before the job ended', False
        ip = inst['networkInterfaces'][0]['accessConfigs'][0]['natIP']
        ssh = ['ssh'] + opts + [f'coreav@{ip}']
        try:
            r = subprocess.run(ssh + ['cat job/job.status 2>/dev/null || { test -e job/job.pid && echo running; } '
                                      '|| echo absent'], capture_output=True, text=True, timeout=120)
            word = r.stdout.strip() if r.returncode == 0 else ''
        except subprocess.TimeoutExpired:
            word = ''
        if word == 'absent':
            return 'the job was never started', False
        if word not in ('', 'running'):
            break
        time.sleep(POLL)
    os.makedirs(s['download'], exist_ok=True)
    for attempt in range(3):
        try:
            ok = (subprocess.run(ssh + ['mkdir -p job/out && cp job/job.log job/job.status job/out/'],
                                 timeout=120).returncode == 0
                  and subprocess.run(['scp', '-q', '-r'] + opts + [f'coreav@{ip}:job/out/.', s['download']],
                                     timeout=max(60, int(s['deadline'] - time.time() + 240))).returncode == 0)
        except subprocess.TimeoutExpired:
            ok = False
        if ok:
            s.update(status=f'job status {word}', came_back=True)
            save(state_dir, s)
            return s['status'], True
        print(f'copying the output failed (attempt {attempt + 1} of 3)', flush=True)
        time.sleep(POLL)
    return f'job status {word}, but its output did not come back whole', False


def finish(gce, state_dir, s, status, came_back):
    """Delete the VM, write spend.json, and record the outcome; the ssh key goes once the VM is gone."""
    name = s['name']
    print('deleting', name, flush=True)
    for attempt in range(5):
        try:
            op = gce.call('DELETE', f'{zone_path(s)}/instances/{name}', ok404=True)
            if op:
                gce.wait(op)
            break
        except (SystemExit, OSError) as e:
            print(f'delete attempt {attempt + 1} failed: {str(e)[:300]}', flush=True)
            time.sleep(30)
    # maxRunDuration deletes the VM at max_hours, so a collect long after that does not count the hours since.
    hours = min(time.time() - s['created'], s['max_hours'] * 3600) / 3600
    spend = {'vm': name, 'machine': s['machine'], 'zone': s['zone'], 'provisioning': s['provisioning'],
             'threads_per_core': s['threads_per_core'], 'hours': round(hours, 3),
             'usd_per_hour_bound': s['usd_per_hour'], 'usd_bound': round(hours * s['usd_per_hour'], 3),
             'status': status}
    os.makedirs(s['download'], exist_ok=True)
    with open(os.path.join(s['download'], 'spend.json'), 'w') as f:
        json.dump(spend, f, indent=1)
    print(json.dumps(spend), flush=True)
    try:
        left = gce.call('GET', f'{zone_path(s)}/instances/{name}', ok404=True) is not None
    except (SystemExit, OSError):
        left = True
    if left:
        print(f'WARNING: {name} may still exist; it deletes itself at maxRunDuration', flush=True)
    s.update(status=status, came_back=came_back, gone=not left)
    save(state_dir, s)
    if not left:
        for n in ('id', 'id.pub'):
            if os.path.exists(os.path.join(state_dir, n)):
                os.remove(os.path.join(state_dir, n))


def collect(gce, state_dir):
    """Finish the job STATE_DIR records: wait for it, copy its output back, delete the VM and write spend.json. Once
    the VM is gone, it only reports the recorded outcome."""
    with open(os.path.join(state_dir, 'state.json')) as f:
        s = json.load(f)
    if s.get('gone'):
        print(f'already collected: {s["status"]}', flush=True)
    else:
        status, came_back = s.get('status', 'collection failed'), s.get('came_back', False)
        try:
            if not came_back:
                status, came_back = fetch(gce, s, state_dir)
        finally:
            finish(gce, state_dir, s, status, came_back)
    if not s['came_back']:
        raise SystemExit(f'no output: {s["status"]}')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--upload', help='directory copied to ~/job on the VM')
    ap.add_argument('--job',
                    help='script inside the upload directory and its arguments, run as `bash JOB` in ~/job (a shell '
                         'word list, as given)')
    ap.add_argument('--download', help='local directory that receives ~/job/out')
    ap.add_argument('--state', help='new or empty directory for the job state and its ssh key (default: DOWNLOAD.gcpjob)')
    ap.add_argument('--collect', metavar='STATE_DIR',
                    help='finish the job of an earlier run whose controller stopped (no other job arguments)')
    ap.add_argument('--zone', default='us-central1-a')
    ap.add_argument('--machine', default='c4-standard-8')
    ap.add_argument('--pmu', default='STANDARD', choices=['ARCHITECTURAL', 'STANDARD', 'ENHANCED'])
    ap.add_argument('--image-family', default='debian-13')
    ap.add_argument('--disk-gb', type=int, default=40)
    ap.add_argument('--max-hours', type=float, default=2.0, help='the VM deletes itself after this')
    ap.add_argument('--usd-per-hour', type=float, help='upper bound on the on-demand hourly price (0.60 for c4-standard-8)')
    ap.add_argument('--cap-usd', type=float, default=2.0)
    ap.add_argument('--provisioning', default='spot', choices=['spot', 'standard'],
                    help='SPOT (default; may be preempted) or STANDARD (on demand)')
    ap.add_argument('--threads-per-core', type=int, choices=[1, 2],
                    help='advancedMachineFeatures.threadsPerCore; 1 turns SMT off (unset: the machine type\'s '
                         'default)')
    ap.add_argument('--turbo-mode', choices=['ALL_CORE_MAX'],
                    help='advancedMachineFeatures.turboMode (unset: the platform\'s default, unrestricted boost)')
    a = ap.parse_args()
    if a.collect:
        if a.upload or a.job or a.download or a.state:
            ap.error('--collect takes no --upload, --job, --download or --state: the state directory has them')
        return collect(Compute(credentials()), a.collect)
    if not (a.upload and a.job and a.download):
        ap.error('--upload, --job and --download are required (or --collect STATE_DIR)')
    if a.usd_per_hour is None:
        if a.machine != 'c4-standard-8':
            raise SystemExit('--usd-per-hour is required with a machine other than c4-standard-8')
        a.usd_per_hour = 0.60
    bound = a.max_hours * a.usd_per_hour
    if bound > a.cap_usd:
        raise SystemExit(f'max-hours * usd-per-hour = {bound:.2f} USD is above the cap of {a.cap_usd:.2f} USD')
    project = os.environ.get('COREAV_GCP_PROJECT', '')
    if not project:
        raise SystemExit('COREAV_GCP_SA_KEY and COREAV_GCP_PROJECT must be set')
    gce = Compute(credentials())
    state_dir = os.path.abspath(a.state or os.path.abspath(a.download) + '.gcpjob')
    if os.path.isdir(state_dir) and os.listdir(state_dir):
        raise SystemExit(f'{state_dir} is not empty: it holds another job (finish it with --collect), or give a new '
                         '--state')
    os.makedirs(state_dir, exist_ok=True)
    os.chmod(state_dir, 0o700)
    # Wherever STATE_DIR is, git never stages its key or the project's name.
    with open(os.path.join(state_dir, '.gitignore'), 'w') as f:
        f.write('*\n')
    z = f'projects/{project}/zones/{a.zone}'
    model = a.provisioning.upper()
    features = {'performanceMonitoringUnit': a.pmu}
    if a.threads_per_core is not None:
        features['threadsPerCore'] = a.threads_per_core
    if a.turbo_mode is not None:
        features['turboMode'] = a.turbo_mode
    image = gce.call('GET', f'projects/debian-cloud/global/images/family/{a.image_family}')['selfLink']
    # A random suffix: two runs started in the same second must not share a name.
    name = f'coreav-pmu-{time.strftime("%Y%m%d-%H%M%S")}-{os.urandom(3).hex()}'
    keyfile = os.path.join(state_dir, 'id')
    subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'coreav', '-f', keyfile], check=True)
    with open(keyfile + '.pub') as f:
        pub = f.read().strip()
    body = {
        'name': name,
        'machineType': f'{z}/machineTypes/{a.machine}',
        'labels': {'purpose': 'coreav-pmu'},
        'disks': [{'boot': True, 'autoDelete': True, 'initializeParams': {
            'sourceImage': image, 'diskSizeGb': str(a.disk_gb), 'diskType': f'{z}/diskTypes/hyperdisk-balanced'}}],
        'networkInterfaces': [{'network': f'projects/{project}/global/networks/default',
                               'accessConfigs': [{'type': 'ONE_TO_ONE_NAT', 'name': 'External NAT'}]}],
        'scheduling': {'provisioningModel': model, 'instanceTerminationAction': 'DELETE',
                       'maxRunDuration': {'seconds': str(int(a.max_hours * 3600))},
                       'automaticRestart': False, 'onHostMaintenance': 'TERMINATE'},
        'advancedMachineFeatures': features,
        'metadata': {'items': [{'key': 'ssh-keys', 'value': f'coreav:{pub}'}]},
    }
    s = {'project': project, 'zone': a.zone, 'name': name, 'machine': a.machine, 'provisioning': model,
         'threads_per_core': a.threads_per_core, 'download': os.path.abspath(a.download), 'max_hours': a.max_hours,
         'usd_per_hour': a.usd_per_hour, 'cap_usd': a.cap_usd, 'created': time.time()}
    s['deadline'] = s['created'] + a.max_hours * 3600 - 300
    smt = '' if a.threads_per_core is None else f', {a.threads_per_core} thread(s) a core'
    turbo = '' if a.turbo_mode is None else f', turbo {a.turbo_mode}'
    print(f'creating {name} ({a.machine}, {a.zone}, {model}, PMU {a.pmu}{smt}{turbo}, max {a.max_hours} h)',
          flush=True)
    print(f'job state in {state_dir}: if this run stops, finish it with --collect {state_dir}', flush=True)
    save(state_dir, s)
    try:
        op = gce.call('POST', f'{z}/instances', body)
    except BaseException as e:
        if ': HTTP 409: ' in str(e):
            # The name in use: no VM was created, and the one of that name is not this run's to delete.
            s.update(status='the name is in use: no VM was created', came_back=False, gone=True)
            save(state_dir, s)
        else:
            finish(gce, state_dir, s, 'the VM was not created', False)
        raise
    try:
        gce.wait(op)
        inst = gce.call('GET', f'{z}/instances/{name}')
        os.makedirs(a.download, exist_ok=True)
        with open(os.path.join(a.download, 'instance.json'), 'w') as f:
            json.dump({'machine_type': inst['machineType'].rsplit('/', 1)[-1], 'zone': inst['zone'].rsplit('/', 1)[-1],
                       'cpu_platform': inst.get('cpuPlatform'),
                       'advanced_machine_features': inst.get('advancedMachineFeatures'),
                       'provisioning': inst.get('scheduling', {}).get('provisioningModel')}, f, indent=1)
        ip = inst['networkInterfaces'][0]['accessConfigs'][0]['natIP']
        opts = ssh_opts(state_dir)
        ssh = ['ssh'] + opts + [f'coreav@{ip}']
        ssh_ready(ssh)
        print('uploading', flush=True)
        subprocess.run(ssh + ['mkdir -p job'], check=True, timeout=120)
        subprocess.run(['scp', '-q', '-r'] + opts + [os.path.join(a.upload, '.'), f'coreav@{ip}:job/'], check=True,
                       timeout=max(60, int(s['deadline'] - time.time())))
        print('running the job', flush=True)
        # Detached from this ssh session, so that the job outlives this controller; collect reads job.status.
        run = (f'timeout {int(s["deadline"] - time.time())} bash {a.job} > job.log 2>&1; '
               'echo $? > job.status.part && mv job.status.part job.status')
        subprocess.run(ssh + [f'cd job && {{ nohup bash -c {shlex.quote(run)} > /dev/null 2>&1 < /dev/null & '
                              'echo $! > job.pid; }'], check=True, timeout=120)
    except BaseException:
        finish(gce, state_dir, s, 'the job did not start', False)
        raise
    return collect(gce, state_dir)


if __name__ == '__main__':
    main()
