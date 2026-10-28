#!/usr/bin/env python3
"""Run one job on a short-lived Compute Engine VM with hardware performance counters, and delete the VM.

    gcp_vm_job.py --upload LOCAL_DIR --job job.sh --download RESULTS_DIR [--max-hours 2] [--cap-usd 2]
                  [--provisioning spot|standard] [--threads-per-core N]

Creates a VM (default a SPOT c4-standard-8 in us-central1-a, Debian, hyperdisk-balanced boot disk) with
`advancedMachineFeatures.performanceMonitoringUnit` set, and `threadsPerCore` where `--threads-per-core` is given
(1 turns simultaneous multithreading off: the VM has one vCPU a physical core, and is billed for the machine's full
vCPU count). `--provisioning standard` makes an on-demand VM, which is not preempted. Copies LOCAL_DIR to `~/job`
on it over SSH with a key made for this run, runs `bash ~/job/JOB` there with a hard timeout, copies `~/job/out` back
to RESULTS_DIR, and deletes the VM in a `finally`. The VM also carries `scheduling.maxRunDuration` with
`instanceTerminationAction=DELETE`, so it deletes itself if this script dies first.

Credentials: COREAV_GCP_SA_KEY (a service-account key, JSON text or a path to it) and COREAV_GCP_PROJECT. The key is
never printed or written anywhere; the access token is kept in memory. Only the Compute Engine REST API (v1) and the
OAuth token endpoint are used, through the standard library; `ssh`, `scp` and `ssh-keygen` must be on PATH.

Spend: the script refuses to start unless `max-hours * usd-per-hour` is at most `cap-usd`. `usd-per-hour` is an upper
bound on the VM's hourly price (0.60 for the default c4-standard-8: its on-demand price in us-central1 plus the boot
disk, rounded up; required for any other machine), so the cap holds whatever the SPOT price is and covers a standard
VM. The bound actually used (hours the VM existed times `usd-per-hour`) is printed at the end and written to
RESULTS_DIR/spend.json.
"""
import argparse, base64, hashlib, json, os, shutil, subprocess, sys, tempfile, time, urllib.error, urllib.parse
import urllib.request

API = 'https://compute.googleapis.com/compute/v1'


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
    def __init__(self, key, project):
        self.key, self.project = key, project
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


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--upload', required=True, help='directory copied to ~/job on the VM')
    ap.add_argument('--job', required=True,
                    help='script inside the upload directory and its arguments, run as `bash JOB` in ~/job (a shell '
                         'word list, as given)')
    ap.add_argument('--download', required=True, help='local directory that receives ~/job/out')
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
    a = ap.parse_args()
    if a.usd_per_hour is None:
        if a.machine != 'c4-standard-8':
            raise SystemExit('--usd-per-hour is required with a machine other than c4-standard-8')
        a.usd_per_hour = 0.60
    bound = a.max_hours * a.usd_per_hour
    if bound > a.cap_usd:
        raise SystemExit(f'max-hours * usd-per-hour = {bound:.2f} USD is above the cap of {a.cap_usd:.2f} USD')
    raw = os.environ.get('COREAV_GCP_SA_KEY', '')
    project = os.environ.get('COREAV_GCP_PROJECT', '')
    if not raw or not project:
        raise SystemExit('COREAV_GCP_SA_KEY and COREAV_GCP_PROJECT must be set')
    key = load_key(raw)
    gce = Compute(key, project)
    p, z = f'projects/{project}', f'projects/{project}/zones/{a.zone}'
    model = a.provisioning.upper()
    features = {'performanceMonitoringUnit': a.pmu}
    if a.threads_per_core is not None:
        features['threadsPerCore'] = a.threads_per_core
    image = gce.call('GET', f'projects/debian-cloud/global/images/family/{a.image_family}')['selfLink']
    # A random suffix: two runs started in the same second must not share a name.
    name = f'coreav-pmu-{time.strftime("%Y%m%d-%H%M%S")}-{os.urandom(3).hex()}'
    tmp = tempfile.mkdtemp(prefix='gcpjob.')
    keyfile = os.path.join(tmp, 'id')
    subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'coreav', '-f', keyfile], check=True)
    pub = open(keyfile + '.pub').read().strip()
    body = {
        'name': name,
        'machineType': f'{z}/machineTypes/{a.machine}',
        'labels': {'purpose': 'coreav-pmu'},
        'disks': [{'boot': True, 'autoDelete': True, 'initializeParams': {
            'sourceImage': image, 'diskSizeGb': str(a.disk_gb), 'diskType': f'{z}/diskTypes/hyperdisk-balanced'}}],
        'networkInterfaces': [{'network': f'{p}/global/networks/default',
                               'accessConfigs': [{'type': 'ONE_TO_ONE_NAT', 'name': 'External NAT'}]}],
        'scheduling': {'provisioningModel': model, 'instanceTerminationAction': 'DELETE',
                       'maxRunDuration': {'seconds': str(int(a.max_hours * 3600))},
                       'automaticRestart': False, 'onHostMaintenance': 'TERMINATE'},
        'advancedMachineFeatures': features,
        'metadata': {'items': [{'key': 'ssh-keys', 'value': f'coreav:{pub}'}]},
    }
    deadline = time.time() + a.max_hours * 3600 - 300
    created = None
    status = 'not started'
    try:
        smt = '' if a.threads_per_core is None else f', {a.threads_per_core} thread(s) a core'
        print(f'creating {name} ({a.machine}, {a.zone}, {model}, PMU {a.pmu}{smt}, max {a.max_hours} h)', flush=True)
        created = time.time()
        try:
            op = gce.call('POST', f'{z}/instances', body)
        except SystemExit as e:
            # The name in use (HTTP 409): no VM was created, and the one of that name is not this run's to delete.
            if 'HTTP 409' in str(e):
                created = None
            raise
        gce.wait(op)
        inst = gce.call('GET', f'{z}/instances/{name}')
        ip = inst['networkInterfaces'][0]['accessConfigs'][0]['natIP']
        opts = ['-i', keyfile, '-o', 'StrictHostKeyChecking=no', '-o', f'UserKnownHostsFile={tmp}/known',
                '-o', 'ConnectTimeout=20', '-o', 'ServerAliveInterval=30', '-o', 'LogLevel=ERROR']
        ssh = ['ssh'] + opts + [f'coreav@{ip}']
        ssh_ready(ssh)
        print('uploading', flush=True)
        subprocess.run(ssh + ['mkdir -p job'], check=True, timeout=120)
        subprocess.run(['scp', '-q', '-r'] + opts + [os.path.join(a.upload, '.'), f'coreav@{ip}:job/'], check=True,
                       timeout=max(60, int(deadline - time.time())))
        print('running the job', flush=True)
        left = int(deadline - time.time())
        r = subprocess.run(ssh + [f'cd job && timeout {left} bash {a.job} > job.log 2>&1; echo $? > job.status'],
                           timeout=left + 60)
        status = f'ssh exit {r.returncode}'
        os.makedirs(a.download, exist_ok=True)
        subprocess.run(ssh + ['mkdir -p job/out && cp job/job.log job/job.status job/out/'], timeout=120)
        subprocess.run(['scp', '-q', '-r'] + opts + [f'coreav@{ip}:job/out/.', a.download],
                       timeout=max(60, int(deadline - time.time() + 240)))
        status += ', job status ' + open(os.path.join(a.download, 'job.status')).read().strip()
    finally:
        if created is not None:
            print('deleting', name, flush=True)
            for attempt in range(5):
                try:
                    op = gce.call('DELETE', f'{z}/instances/{name}', ok404=True)
                    if op:
                        gce.wait(op)
                    break
                except (SystemExit, OSError) as e:
                    print(f'delete attempt {attempt + 1} failed: {str(e)[:300]}', flush=True)
                    time.sleep(30)
            hours = (time.time() - created) / 3600
            spend = {'vm': name, 'machine': a.machine, 'zone': a.zone, 'provisioning': model,
                     'threads_per_core': a.threads_per_core, 'hours': round(hours, 3),
                     'usd_per_hour_bound': a.usd_per_hour, 'usd_bound': round(hours * a.usd_per_hour, 3),
                     'status': status}
            os.makedirs(a.download, exist_ok=True)
            json.dump(spend, open(os.path.join(a.download, 'spend.json'), 'w'), indent=1)
            print(json.dumps(spend), flush=True)
            try:
                left = gce.call('GET', f'{z}/instances/{name}', ok404=True) is not None
            except (SystemExit, OSError):
                left = True
            if left:
                print(f'WARNING: {name} may still exist; it deletes itself at maxRunDuration', flush=True)
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    main()
