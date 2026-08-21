#!/usr/bin/env python3
"""Low-core DashMap hang reproducer + watchdog.

Makes CI-like DashMap shard collisions reproducible and converts silent
hangs into bounded failures. The companion static analyzer
(tools/dashmap_guard) is the primary prevention gate; this runner is the
secondary low-core evidence layer.

Usage:

  # run a whole cargo test suite pinned to low CPU count with a watchdog
  python3 tools/low_core_test.py \\
      --cpus 0,1 --timeout-seconds 1800 -- \\
      cargo test --all-targets -- --test-threads=1

  # enumerate Rust tests and run each individually (diagnostic mode)
  python3 tools/low_core_test.py \\
      --enumerate-tests --cpus 0,1 --per-test-timeout-seconds 120

  # watchdog self-test (short timeout, hangs on purpose, bounded)
  python3 tools/low_core_test.py --self-test

Options:
  --cpus LIST                 CPU list for taskset (-c), e.g. 0 or 0,1
  --timeout-seconds N         whole-suite watchdog (default 1800)
  --per-test-timeout-seconds  N per-test watchdog in --enumerate-tests mode
  --log-dir PATH              where logs go (default target/dashmap-lowcore-logs)
  --enumerate-tests           discover and run every test individually
  --self-test                 verify the watchdog kills a hanging child

Classification: PASS / TEST_FAILURE / TIMEOUT_OR_HANG / INFRA_UNSUPPORTED.
"""

import argparse
import collections
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time

def python_interpreter():
    """Return a real CPython usable for the hang fixture.

    In some IDE/agent shells ``sys.executable`` is the host binary (e.g. an
    AppImage), not Python. Spawning that would not hang and would poison the
    watchdog self-test. Prefer ``python3`` from PATH when it actually runs.
    """
    candidates = []
    which_py = shutil.which("python3") or shutil.which("python")
    if which_py:
        candidates.append(which_py)
    if sys.executable:
        candidates.append(sys.executable)
    seen = set()
    for cand in candidates:
        if not cand or cand in seen:
            continue
        seen.add(cand)
        base = os.path.basename(cand).lower()
        if "cursor" in base or cand.endswith(".appimage"):
            continue
        try:
            out = subprocess.run(
                [cand, "-c", "print('ok')"],
                capture_output=True,
                text=True,
                timeout=5,
            )
        except (OSError, subprocess.TimeoutExpired):
            continue
        if out.returncode == 0 and out.stdout.strip() == "ok":
            return cand
    raise RuntimeError("no usable python3 interpreter for the watchdog self-test")

CLASS_PASS = "PASS"
CLASS_FAIL = "TEST_FAILURE"
CLASS_HANG = "TIMEOUT_OR_HANG"
CLASS_INFRA = "INFRA_UNSUPPORTED"

TAIL_LINES = 800          # lines kept in memory for the "last stdout/stderr"
SIGKILL_GRACE_SECS = 5    # wait after SIGTERM before SIGKILL


def logdir(args):
    return args.log_dir


def taskset_prefix(cpus):
    """Return the taskset argv prefix, or None if pinning is unavailable."""
    if cpus is None:
        return None
    if shutil.which("taskset") is None:
        return None
    return ["taskset", "-c", str(cpus)]


def classify_exit(rc, timed_out, pinned):
    if not pinned:
        return CLASS_INFRA
    if timed_out:
        return CLASS_HANG
    if rc == 0:
        return CLASS_PASS
    return CLASS_FAIL


def allowed_cpus():
    """Return the set of CPU indices this process may use."""
    if hasattr(os, "sched_getaffinity"):
        try:
            return set(os.sched_getaffinity(0))
        except OSError:
            pass
    count = os.cpu_count() or 1
    return set(range(count))


def parse_cpu_list(cpus: str):
    out = set()
    for part in cpus.split(","):
        part = part.strip()
        if not part:
            continue
        out.add(int(part))
    return out


def validate_cpus(cpus: str):
    requested = parse_cpu_list(cpus)
    allowed = allowed_cpus()
    missing = sorted(requested - allowed)
    if missing:
        print("INFRA_UNSUPPORTED: requested CPUs are not available in this environment", file=sys.stderr)
        print(f"  requested CPUs: {cpus}", file=sys.stderr)
        print(f"  allowed CPUs: {','.join(str(c) for c in sorted(allowed))}", file=sys.stderr)
        print(f"  platform: {sys.platform}", file=sys.stderr)
        print(f"  taskset: {shutil.which('taskset') or 'unavailable'}", file=sys.stderr)
        return False
    return True


def list_descendant_pids(root_pid):
    """Best-effort enumeration of descendant processes in the session tree."""
    out = []
    try:
        proc = subprocess.run(
            ["ps", "-eo", "pid,ppid,comm", "--no-headers"],
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return out
    children = {}
    for line in proc.stdout.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) < 3:
            continue
        pid, ppid, comm = int(parts[0]), int(parts[1]), parts[2]
        children.setdefault(ppid, []).append((pid, comm))
    stack = [root_pid]
    seen = set()
    while stack:
        pid = stack.pop()
        for child_pid, comm in children.get(pid, []):
            if child_pid in seen:
                continue
            seen.add(child_pid)
            out.append((child_pid, comm))
            stack.append(child_pid)
    return out


def collect_proc_diagnostics(pid, log_path):
    """Best-effort capture of kernel scheduler/stack info for a hung pid."""
    lines = []
    # per-thread wchan + stat via procfs
    task_dir = f"/proc/{pid}/task"
    if os.path.isdir(task_dir):
        try:
            for tid in sorted(os.listdir(task_dir), key=int):
                wchan = "<n/a>"
                try:
                    with open(f"{task_dir}/{tid}/wchan") as f:
                        wchan = f.read().strip() or "<unknown>"
                except Exception:
                    pass
                stack = ""
                if os.access(f"{task_dir}/{tid}/stack", os.R_OK):
                    try:
                        with open(f"{task_dir}/{tid}/stack") as f:
                            stack = f.read().strip()
                    except Exception:
                        pass
                lines.append(f"[tid {tid}] wchan={wchan}")
                if stack:
                    lines.append(f"[tid {tid}] stack:\n{stack}")
        except Exception as e:  # noqa: BLE001
            lines.append(f"[procfs] error reading {task_dir}: {e}")
    # thread state via ps (does not require root)
    try:
        out = subprocess.run(
            ["ps", "-L", "-p", str(pid), "-o", "tid=,wchan=,stat=,comm="],
            capture_output=True,
            text=True,
            timeout=10,
        )
        lines.append("[ps] " + (out.stdout or out.stderr or "").strip())
    except Exception as e:  # noqa: BLE001
        lines.append(f"[ps] unavailable: {e}")
    try:
        with open(log_path, "a") as f:
            f.write("===== hang diagnostics =====\n")
            f.write("\n".join(lines) + "\n")
            f.write("==============================\n")
    except Exception as e:  # noqa: BLE001
        lines.append(f"[log] cannot append: {e}")
    return lines


class TailBuffer:
    """Keeps the last N chars (approx) of a stream for the final report."""

    def __init__(self, limit=60_000):
        self.buf = collections.deque()
        self.limit = limit

    def write(self, text):
        if not text:
            return
        self.buf.append(text)
        total = sum(len(x) for x in self.buf)
        while total > self.limit and self.buf:
            total -= len(self.buf[0])
            self.buf.popleft()

    def getvalue(self):
        return "".join(self.buf)


def start_child(cmd, cpus, log_file):
    base_cmd = cmd
    argv = []
    if cpus is not None:
        prefix = taskset_prefix(cpus)
        if prefix is None:
            return None, "taskset unavailable; cannot pin CPUs"
        argv += prefix
    argv += base_cmd
    # new session/process group so we can kill the whole tree later
    stdout = open(log_file, "wb", buffering=0)
    p = subprocess.Popen(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=stdout,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        bufsize=0,
    )
    return p, None


def run_once(cmd, cpus, timeout_secs, log_path, tail):
    """Run cmd under taskset + process group + watchdog; returns (class, rc, elapsed)."""
    start = time.time()
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    child, err = start_child(cmd, cpus, log_path)
    if child is None:
        with open(log_path, "a") as f:
            f.write(str(err) + "\n")
        return CLASS_INFRA, None, time.time() - start
    if cpus is not None and not validate_cpus(cpus):
        child.kill()
        with open(log_path, "a") as f:
            f.write("INFRA_UNSUPPORTED: CPU affinity rejected\n")
        return CLASS_INFRA, None, time.time() - start

    timed_out = False
    rc = None
    try:
        rc = child.wait(timeout=timeout_secs)
    except subprocess.TimeoutExpired:
        timed_out = True

    if timed_out:
        for child_pid, comm in list_descendant_pids(child.pid):
            collect_proc_diagnostics(child_pid, log_path)
            try:
                with open(log_path, "a") as f:
                    f.write(f"[descendant] pid={child_pid} ppid={child.pid} comm={comm}\n")
            except OSError:
                pass
        collect_proc_diagnostics(child.pid, log_path)
        # terminate the whole process tree (session leaders own their group)
        try:
            os.killpg(os.getpgid(child.pid), signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            child.wait(timeout=SIGKILL_GRACE_SECS)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(os.getpgid(child.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
            try:
                child.wait(timeout=SIGKILL_GRACE_SECS)
            except subprocess.TimeoutExpired:
                pass

    elapsed = time.time() - start

    if not tail:
        tail = TailBuffer()
    try:
        with open(log_path, "r", errors="replace") as f:
            tail.write(f.read())
    except OSError:
        pass

    pinned = taskset_prefix(cpus) is not None
    return classify_exit(rc, timed_out, pinned), rc, elapsed


def summary_line(kind, name, elapsed, rc):
    return f"{kind:13} {name}  [{elapsed:7.1f}s rc={rc}]"


def run_suite(args):
    cmd = args.command
    label = " ".join(cmd) if cmd else "(empty command)"
    run = {"command": cmd, "cpus": args.cpus, "started": time.time()}
    pin = taskset_prefix(args.cpus) is not None

    log_path = os.path.join(logdir(args), "suite.log")
    kind, rc, elapsed = run_once(cmd, args.cpus, args.timeout_seconds, log_path, None)

    print(summary_line(kind, label, elapsed, rc))
    print(f"  command : {' '.join(cmd) if cmd else ''}")
    print(f"  cpuset  : {args.cpus if args.cpus is not None else '(not pinned)'}")
    print(f"  started : {run['started']}")
    print(f"  ended   : {time.time()}")
    print(f"  elapsed : {elapsed:.1f}s")
    print(f"  exit     : {rc}")
    print(f"  log     : {log_path}")
    if kind == CLASS_HANG:
        print("  last stdout/stderr (tail):")
        for line in tail_text(log_path).splitlines()[-80:]:
            print("   ", line)
    return 0 if kind == CLASS_PASS else 1


def tail_text(log_path):
    try:
        with open(log_path, "r", errors="replace") as f:
            data = f.read()
        return data[-60_000:]
    except OSError:
        return ""


def discover_test_binaries():
    """cargo test --no-run --message-format=json -> executable paths."""
    proc = subprocess.run(
        ["cargo", "test", "--no-run", "--all-targets", "--message-format=json"],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        print("cargo test --no-run failed to build:", file=sys.stderr)
        print(proc.stderr[-4000:], file=sys.stderr)
        return None
    bins = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        if obj.get("reason") != "compiler-artifact":
            continue
        exe = obj.get("executable")
        if not exe:
            continue
        # cargo test --no-run emits an executable for every test harness
        # (lib, bin, integration, example). Do NOT filter on target.kind:
        # library tests have kind=["lib"] and include the historical
        # getblock_and_setblock_compile_and_execute hang site.
        bins.append(exe)
    return sorted(set(bins))


def list_tests(binary):
    proc = subprocess.run([binary, "--list"], capture_output=True, text=True)
    names = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        # output: "name: test" or "name: bench"
        m = re.match(r"^(.+?): (test|bench)$", line)
        if m:
            names.append((m.group(1), m.group(2)))
    return names


def run_enumerated(args):
    bins = discover_test_binaries()
    if bins is None:
        return 1
    results = []
    overall = 0
    for binary in bins:
        tests = list_tests(binary)
        if not tests:
            continue
        slug = os.path.basename(binary)
        for (name, kind) in tests:
            cmd = [binary, name, "--exact", "--nocapture"]
            log_path = os.path.join(
                logdir(args),
                "tests",
                f"{slug}__{name.replace('/', '_').replace('::', '__')}.log",
            )
            tag = name
            run_path = os.path.join(logdir(args), "per-test")
            os.makedirs(run_path, exist_ok=True)
            kind_c, rc, elapsed = run_once(
                cmd, args.cpus, args.per_test_timeout_seconds, log_path, None
            )
            line = summary_line(kind_c, tag, elapsed, rc)
            results.append(line)
            if kind_c not in (CLASS_PASS,):
                overall = 1
    print("\n===== deterministic per-test summary =====")
    for line in sorted(results):
        print(line)
    return overall


SELF_TEST_FIXTURE = r'''
import time
def main():
    print("fixture started, intending to hang")
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass
main()
'''


def run_self_test(args):
    """Verify the watchdog: fixture hangs, timeout fires, group killed, logs."""
    print("running watchdog self-test (fixture sleeps forever, 3s timeout)...")
    fixture_dir = os.path.join(logdir(args), "self-test")
    os.makedirs(fixture_dir, exist_ok=True)
    fixture = os.path.join(fixture_dir, "hang_fixture.py")
    with open(fixture, "w") as f:
        f.write(SELF_TEST_FIXTURE)

    # spawn a grandchild from the fixture so we can prove the whole process
    # group is killed, not just the direct child.
    py = python_interpreter()
    wrapper = os.path.join(fixture_dir, "wrapper.py")
    with open(wrapper, "w") as f:
        f.write(
            "import subprocess\n"
            "p = subprocess.Popen([%r, %r])\n"
            "print('wrapper pid', p.pid, flush=True)\n"
            "p.wait()\n"
            % (py, fixture)
        )

    cmd = [py, wrapper]
    log_path = os.path.join(logdir(args), "self-test", "run.log")
    kind, rc, elapsed = run_once(cmd, args.cpus, 3, log_path, None)
    print(summary_line(kind, "hang_fixture", elapsed, rc))
    print(f"  log: {log_path}")

    # assertions
    diag = tail_text(log_path)
    checks = []
    checks.append(("timeout fired (TIMEOUT_OR_HANG)", kind == CLASS_HANG))
    checks.append(("logs written", os.path.exists(log_path) and os.path.getsize(log_path) > 0))
    checks.append(("hang diagnostics captured", "wchan" in diag))
    # ensure no fixture process survived (group kill)
    leftover = subprocess.run(
        ["pgrep", "-f", "hang_fixture\\.py"],
        capture_output=True,
        text=True,
    )
    checks.append(("process group killed (no leftover fixture)", leftover.returncode != 0))

    ok = True
    for label, result in checks:
        print(f"  [{'ok' if result else 'FAIL'}] {label}")
        ok = ok and result
    return 0 if ok else 1


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cpus", default=None, help="taskset CPU list, e.g. 0 or 0,1")
    parser.add_argument("--timeout-seconds", type=int, default=1800)
    parser.add_argument("--per-test-timeout-seconds", type=int, default=120)
    parser.add_argument(
        "--log-dir",
        default=os.path.join("target", "dashmap-lowcore-logs"),
    )
    parser.add_argument("--enumerate-tests", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("command", nargs="*", help="command after --")
    return parser.parse_args(argv)


def main():
    argv = list(sys.argv[1:])
    if "--" in argv:
        idx = argv.index("--")
        cmd = argv[idx + 1 :]
        argv = argv[:idx]
    else:
        cmd = []
    args = parse_args(argv)
    args.command = cmd

    if args.self_test:
        return run_self_test(args)
    if args.enumerate_tests:
        if not args.cpus:
            print("--enumerate-tests requires --cpus (<0 or 0,1>)", file=sys.stderr)
            return 2
        return run_enumerated(args)
    return run_suite(args)


if __name__ == "__main__":
    sys.exit(main())
