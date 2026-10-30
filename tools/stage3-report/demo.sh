#!/usr/bin/env bash
# The hostile-file demo (docs/design/stage3-report.md): the same
# uploaded file reaches a decoder in default FFmpeg, and is rejected by
# CoreAV at probe time, on the command line.
#
#   tools/stage3-report/demo.sh [FILE] [OPTION...]
#
# Without FILE, the demo makes `upload.mp4`: a PPM image named like a video
# (the route of CVE-2024-7055, a heap overflow in FFmpeg's PNM decoder up to
# 7.0.1), with the pinned ffmpeg as a black box. Any file of the hostile
# suite works too (tools/hostile/generate.py).
#
# FFmpeg runs as a black box in a sandbox: user, mount, network, process,
# IPC and host name namespaces of its own (no network interface, no shared
# memory or message queue of the host); a new session without a
# controlling terminal; a read-only root built from an allow list, holding
# only the system's programs, libraries and configuration (/usr, /etc,
# /bin, /sbin, /lib*), the ffmpeg binary, a /dev of null, zero, random and
# urandom, an empty /proc and the input file, read-only, at
# /tmp/input/file, so no user, service or session data, socket, terminal or
# other device of the host is in it; an empty environment; every capability
# dropped and no new privileges (setpriv); limits on address space (1 GiB),
# CPU time (10 s), file size and processes, and a 20 s wall-clock timeout.
# The demo checks the sandbox first (no capabilities, no terminal, no file
# write, no Unix socket, no credentials, no processes, no shared memory, no
# inherited descriptor, and nothing in the root, /dev or /tmp beyond the
# allow list) and refuses to run FFmpeg otherwise.
#
# The options are those of a `demo-extra` script beside this one, when this
# tree has one: it is sourced at the end, with the options in EXTRA_OPTIONS,
# and shows the same file to further engines.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
COREAV="${COREAV:-$ROOT/target/release/coreav}"
FFMPEG="${FFMPEG:-$(command -v ffmpeg || true)}"
POLICY="${POLICY:-$ROOT/docs/schema/policy-examples/web-upload.json}"
EXTRA="$(dirname "${BASH_SOURCE[0]}")/demo-extra"
FILE=""
EXTRA_OPTIONS=()
for arg in "$@"; do
  case "$arg" in
    -*) if [ -f "$EXTRA" ]; then EXTRA_OPTIONS+=("$arg")
        else echo "usage: demo.sh [FILE]" >&2; exit 2; fi ;;
    *) FILE="$arg" ;;
  esac
done
[ -x "$COREAV" ] || { echo "build coreav first: cargo build --release -p coreav-cli" >&2; exit 2; }
[ -n "$FFMPEG" ] || { echo "ffmpeg not found (scripts/install-oracle-ffmpeg.sh)" >&2; exit 2; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
if [ -z "$FILE" ]; then
  FILE="$WORK/upload.mp4"
  "$FFMPEG" -hide_banner -loglevel error -f lavfi -i color=c=red:s=64x64:d=1 -frames:v 1 -c:v ppm \
    -f image2pipe "$FILE"
fi
FILE="$(cd "$(dirname "$FILE")" && pwd)/$(basename "$FILE")"

section() { printf '\n== %s\n' "$1"; }

section "the file: $(basename "$FILE"), $(wc -c < "$FILE") bytes, sha256 $(sha256sum "$FILE" | cut -c1-16)…"

# The sandbox: runs "$@" with no network, no IPC, process or host name
# namespace shared with the host, and no controlling terminal, in an empty
# root that holds only, read-only: the system's programs, libraries and
# configuration (/usr, /etc, and /bin, /sbin and /lib* as the host has
# them), the ffmpeg binary, a /dev of null, zero, random and urandom, an
# empty /proc, and /tmp with the input file at /tmp/input/file; with an
# empty environment and no capabilities. Nothing else of the host is in it
# (an allow list): no user, service or session data, socket, terminal or
# other device.
FFMPEG="$(readlink -f "$FFMPEG")"
ROOT_DIR="$WORK/root"
mkdir "$ROOT_DIR"
sandbox() {
  unshare --user --map-root-user --mount --net --pid --ipc --uts --fork \
    sh -s -- "$ROOT_DIR" "$FILE" "$FFMPEG" "$@" <<'EOF'
set -e
R="$1"; FILE="$2"; FFMPEG="$3"; shift 3
mount --make-rprivate /
mount -t tmpfs -o size=1m,mode=0755 tmpfs "$R"
for d in usr etc; do
  mkdir "$R/$d"
  mount --rbind "/$d" "$R/$d"
done
# /bin, /sbin and /lib*: the same links into /usr where the host has them
# so (a merged /usr), else bound.
for d in bin sbin lib lib32 lib64 libx32; do
  if [ -L "/$d" ]; then
    ln -s "$(readlink "/$d")" "$R/$d"
  elif [ -d "/$d" ]; then
    mkdir "$R/$d"
    mount --rbind "/$d" "$R/$d"
  fi
done
# The ffmpeg binary alone, at its own path, unless /usr holds it already.
case "$FFMPEG" in
  /usr/*) ;;
  *) mkdir -p "$R$(dirname "$FFMPEG")"; : > "$R$FFMPEG"; mount --bind "$FFMPEG" "$R$FFMPEG" ;;
esac
mkdir "$R/dev" "$R/proc" "$R/tmp" "$R/tmp/input"
for n in null zero random urandom; do
  : > "$R/dev/$n"
  mount --bind "/dev/$n" "$R/dev/$n"
done
: > "$R/tmp/input/file"
mount --bind "$FILE" "$R/tmp/input/file"
# Every mount under the root read-only, then the root itself.
for m in $(awk -v r="$R" 'index($2, r"/")==1 {print $2}' /proc/self/mounts | sort -u); do
  mount -o remount,bind,ro "$m"
done
mount -o remount,bind,ro "$R"
if [ "$(grep -c -v '^ *lo:' /proc/net/dev)" -gt 2 ]; then echo "sandbox: a network interface is present" >&2; exit 97; fi
hostname sandbox
# No descriptor of the launcher's beyond 0 to 2 goes in.
for fd in $(ls /proc/self/fd); do
  if [ "$fd" -gt 2 ]; then eval "exec $fd>&-" 2>/dev/null || true; fi
done
# setsid: a new session without a controlling terminal, so nothing inside
# can push input to the terminal the demo runs in (TIOCSTI).
exec env -i PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin chroot "$R" \
  setsid --wait setpriv --bounding-set=-all --inh-caps=-all --ambient-caps=-all --no-new-privs "$@"
EOF
}
# The sandbox must hold before FFmpeg runs: no capabilities, no terminal,
# no file write, no Unix socket, no credentials in the environment, no
# process information, no shared memory segment, no inherited descriptor
# beyond 0 to 2, not the host's name, and nothing at the top of the root,
# in /dev or in /tmp but what the allow list puts there.
FFMPEG_TOP="$(echo "$FFMPEG" | cut -d/ -f2)"
sandbox_holds() {
  # An empty bounding set leaves no permitted or effective capability after
  # exec; no_new_privs keeps it so.
  sandbox sh -c 'setpriv --dump | grep -q "^Capability bounding set: \[none\]" || exit 1
                 setpriv --dump | grep -q "^no_new_privs: 1" || exit 1
                 if [ -t 0 ] || [ -t 1 ] || [ -t 2 ] || ( : < /dev/tty ) 2>/dev/null; then exit 1; fi
                 if ( : > /tmp/.coreav-demo-write ) 2>/dev/null; then exit 1; fi
                 [ -z "$(find / -path /proc -prune -o -type s -print 2>/dev/null | head -n 1)" ] || exit 1
                 env | grep -q -E "KEY|SECRET|TOKEN|PASSWORD" && exit 1
                 [ -z "$(ls -A /proc)" ] || exit 1
                 if command -v ipcs > /dev/null && ipcs -m | grep -q "^0x"; then exit 1; fi
                 for entry in $(ls -A /); do
                   case "$entry" in
                     usr|etc|bin|sbin|lib|lib32|lib64|libx32|dev|proc|tmp|'"$FFMPEG_TOP"') ;;
                     *) exit 1 ;;
                   esac
                 done
                 [ "$(ls -A /dev | sort | tr "\n" " ")" = "null random urandom zero " ] || exit 1
                 [ "$(ls -A /tmp)" = "input" ] || exit 1
                 for n in 3 4 5 6 7 8 9; do
                   if ( eval "true <&$n" ) 2>/dev/null; then exit 1; fi
                 done
                 [ "$(hostname)" = sandbox ] || exit 1
                 exit 0' < /dev/null > /dev/null 2>&1
}
if ! sandbox_holds; then
  echo "the sandbox (unshare, an allow-list root, setsid, setpriv) cannot be set up or does not hold here; FFmpeg is not run" >&2
  exit 3
fi

section "FFmpeg 7.0.2, default settings, in the sandbox: ffmpeg -i FILE -f null -"
set +e
ff_out="$(sandbox prlimit --as=$((1 << 30)) --cpu=10 --fsize=0 --nproc=256 \
  timeout 20 "$FFMPEG" -hide_banner -nostdin -i /tmp/input/file -f null - 2>&1)"
ff_status=$?
set -e
printf '%s\n' "$ff_out" | grep -E "^Input #|Stream #|Stream mapping|\(native\)|^\[out#0.*video:|frame=" | tail -n 8 || true
echo "exit status $ff_status"
if printf '%s\n' "$ff_out" | grep -q "frame=" ; then
  echo "-> FFmpeg detected the format from the content and ran its decoder on the file."
fi

section "coreav probe --strict FILE"
set +e
"$COREAV" probe --strict "$FILE" > "$WORK/probe.out" 2> "$WORK/probe.err"
status=$?
set -e
cat "$WORK/probe.err"; head -c 400 "$WORK/probe.out"
echo "exit status $status (1: rejected; no decoder was reached)"

section "coreav check --policy $(basename "$POLICY") FILE"
set +e
"$COREAV" check --policy "$POLICY" "$FILE" > "$WORK/check.out" 2>&1
status=$?
set -e
python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); print("verdict:", v["verdict"]); [print(" -", r["code"] + ":", r["message"]) for r in v["reasons"]]' "$WORK/check.out"
echo "exit status $status (1: the upload is rejected)"

if [ -f "$EXTRA" ]; then
  # shellcheck source=/dev/null
  . "$EXTRA"
fi
