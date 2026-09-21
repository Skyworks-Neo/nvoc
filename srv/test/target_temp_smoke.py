"""target-temp smoke test for the nvoc-srv closed-loop control plane.

Exercises the ``--target-temp`` consumer plane end to end (the part the
author has NOT live-tested): discovery (adopt / spawn / port-occupied),
engagement (``POST /pid?target_c`` then ``POST /mode?value=pid``), closed-loop
convergence under load, idle release, and teardown (``/restore`` + shutdown of
a spawned child). Everything talks to the documented HTTP API; no GPU code is
invoked from this script.

Two modes:

* **direct** (default) -- this script owns the srv lifecycle. It probes the
  port, spawns ``nvoc-srv --foreground`` if nothing listens, engages PID,
  optionally runs a load command, monitors ``/status``, then restores and
  shuts the spawned srv down::

      python srv/test/target_temp_smoke.py \
          --load-cmd "cli-stressor-cuda-rs --profile standard --duration 150" \
          --target-c 70

* **--via-cmd** -- the wrapper owns the session (optimizer or stressor debug
  channel). The port must be free at start; the script monitors ``/status``
  while the wrapper runs, then verifies the wrapper's teardown (mode back to
  auto AND the port freed -- the wrapper spawned the srv, so it must stop it)::

      python srv/test/target_temp_smoke.py \
          --via-cmd "nvoc-auto-optimizer optimize --target-temp 70 ..." \
          --expect-target 70

Exit codes: 0 = all checks passed, 1 = one or more checks failed,
2 = environment error (port occupied, spawn failed, auth required).

Requires only the Python standard library. Mutations always carry the
``X-Requested-With: XMLHttpRequest`` CSRF header, like the web console.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

CSRF = {"X-Requested-With": "XMLHttpRequest"}
EXIT_PASS, EXIT_FAIL, EXIT_ENV = 0, 1, 2

READY_TIMEOUT_S = 15.0
POLL_S = 1.0
IO_TIMEOUT_S = 5.0
TEARDOWN_TIMEOUT_S = 15.0


class EnvError(Exception):
    """Environment problem (port, spawn, auth) -- not a controller failure."""


class SrvHttpError(Exception):
    def __init__(self, code: int, body: str) -> None:
        super().__init__(f"HTTP {code}: {body.strip()[:200]}")
        self.code = code


class Srv:
    """Minimal client for the loopback control plane (mirrors srv_client.rs)."""

    def __init__(self, port: int, auth: str | None) -> None:
        self.port = port
        self.url = f"http://127.0.0.1:{port}"
        self.basic = (
            "Basic " + base64.b64encode(auth.encode()).decode() if auth else None
        )

    def _request(self, method: str, path: str) -> tuple[int, str]:
        req = urllib.request.Request(self.url + path, method=method)
        if method == "POST":
            for key, value in CSRF.items():
                req.add_header(key, value)
        if self.basic:
            req.add_header("Authorization", self.basic)
        try:
            with urllib.request.urlopen(req, timeout=IO_TIMEOUT_S) as resp:
                return resp.status, resp.read().decode(errors="replace")
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode(errors="replace")

    def status(self) -> dict:
        """GET /status as JSON. Raises EnvError on 401 (auth hint)."""
        code, body = self._request("GET", "/status")
        if code == 401:
            raise EnvError(
                "srv requires authentication (auth=auto/on) -- pass --auth USER:PASS "
                'or set auth = "off" in the srv config'
            )
        if code != 200:
            raise SrvHttpError(code, body)
        try:
            return json.loads(body)
        except json.JSONDecodeError as e:
            raise SrvHttpError(code, f"/status is not JSON: {e}") from e

    def post(self, path: str) -> str:
        code, body = self._request("POST", path)
        if code == 401:
            raise EnvError(
                "srv requires authentication (auth=auto/on) -- pass --auth USER:PASS "
                'or set auth = "off" in the srv config'
            )
        if code != 200:
            raise SrvHttpError(code, body)
        return body


def probe(srv: Srv) -> tuple[str, str]:
    """Tri-state probe, same semantics as auto-optimizer/src/srv_client.rs:
    connection refused -> absent, valid /status with empty gpus -> starting,
    valid /status with GPUs -> ready, anything else -> occupied."""
    try:
        st = srv.status()
    except EnvError:
        raise
    except urllib.error.URLError as e:
        if isinstance(e.reason, ConnectionRefusedError):
            return "absent", "nothing listening"
        return "occupied", f"port answered but is not a healthy nvoc-srv ({e})"
    except SrvHttpError as e:
        return "occupied", f"/status answered {e}"
    except Exception as e:  # noqa: BLE001 -- connection-level, mirrors probe_state
        return "occupied", f"port answered but is not a healthy nvoc-srv ({e})"
    gpus = st.get("gpus")
    if not isinstance(gpus, list):
        return "occupied", "/status JSON has no 'gpus' array"
    return ("ready", "control loop live") if gpus else ("starting", "discovering GPUs")


def find_srv_exe(explicit: str | None) -> str:
    if explicit:
        if os.path.exists(explicit):
            return explicit
        raise EnvError(f"--srv-exe not found: {explicit}")
    candidates = ["nvoc-srv"] + (["nvoc-srv.exe"] if os.name == "nt" else [])
    for path in os.environ.get("PATH", "").split(os.pathsep):
        for name in candidates:
            full = os.path.join(path, name)
            if os.path.isfile(full):
                return full
    local = os.path.join(
        "target", "release", "nvoc-srv.exe" if os.name == "nt" else "nvoc-srv"
    )
    if os.path.isfile(local):
        return local
    raise EnvError("nvoc-srv executable not found -- pass --srv-exe or build it first")


def kill_tree(proc: subprocess.Popen) -> None:
    if proc.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
            capture_output=True,
            check=False,
        )
    else:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


class Checks:
    def __init__(self) -> None:
        self.items: list[tuple[str, bool, bool, str]] = []  # name, ok, soft, detail

    def add(self, name: str, ok: bool, detail: str, soft: bool = False) -> None:
        self.items.append((name, ok, soft, detail))

    @property
    def failed(self) -> bool:
        return any(not ok for _, ok, soft, _ in self.items if not soft)

    def report(self) -> None:
        print("\n== checks ==")
        for name, ok, soft, detail in self.items:
            mark = "ok  " if ok else ("warn" if soft else "FAIL")
            print(f"  [{mark}] {name}: {detail}")


def safe_status(srv: Srv) -> dict | None:
    """/status that tolerates the srv vanishing mid-poll (teardown races).
    EnvError (auth) still propagates -- it must surface."""
    try:
        return srv.status()
    except (SrvHttpError, OSError):
        return None


def gpu_of(status: dict, index: int) -> dict | None:
    for gpu in status.get("gpus", []):
        if gpu.get("index") == index:
            return gpu
    return None


def zone_histogram(samples: list[dict]) -> str:
    counts: dict[str, int] = {}
    for sample in samples:
        zone = sample["failsafe"] or "unknown"
        counts[zone] = counts.get(zone, 0) + 1
    return ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "no samples"


def convergence(
    samples: list[dict], target: float, tolerance: float
) -> tuple[float, float | None]:
    """(in-band fraction, seconds to first in-band sample). None = never."""
    in_band = [
        s
        for s in samples
        if s["temp"] is not None and abs(s["temp"] - target) <= tolerance
    ]
    frac = len(in_band) / len(samples) if samples else 0.0
    first = in_band[0]["t"] if in_band else None
    return frac, first


def temp_stats(samples: list[dict]) -> str:
    temps = sorted(s["temp"] for s in samples if s["temp"] is not None)
    if not temps:
        return "no temperature readings"
    return (
        f"min {temps[0]:.1f}  avg {sum(temps) / len(temps):.1f}  "
        f"max {temps[-1]:.1f}  degC"
    )


def monitor(srv: Srv, gpu_index: int, stop, poll: float) -> list[dict]:
    """Poll /status until stop() is truthy. Collects one dict per tick."""
    samples: list[dict] = []
    start = time.monotonic()
    while not stop():
        st = safe_status(srv)
        if st is None:
            samples.append({
                "t": time.monotonic() - start,
                "temp": None,
                "written": None,
                "failsafe": "http error",
                "pid": None,
                "output": None,
                "mode": None,
                "target": None,
            })
            time.sleep(poll)
            continue
        gpu = gpu_of(st, gpu_index)
        if gpu is None:
            time.sleep(poll)
            continue
        pid = gpu.get("pid")
        samples.append({
            "t": time.monotonic() - start,
            "temp": gpu.get("temp_c"),
            "written": gpu.get("fan_written_percent"),
            "failsafe": gpu.get("failsafe"),
            "pid": pid is not None,
            "output": (pid or {}).get("output_percent"),
            "mode": st.get("mode"),
            "target": st.get("target_c"),
        })
        time.sleep(poll)
    return samples


def analyze_loop_alive(samples: list[dict], checks: Checks) -> None:
    engaged = [s for s in samples if s["mode"] == "pid"]
    if not engaged:
        checks.add("loop-alive", False, "no samples with mode=pid")
        return
    pid_frac = sum(1 for s in engaged if s["pid"]) / len(engaged)
    checks.add(
        "loop-alive",
        pid_frac >= 0.9,
        f"PID ran on {pid_frac:.0%} of engaged samples ({len(engaged)} samples)",
    )
    outputs = {round(s["output"], 2) for s in engaged if s["output"] is not None}
    checks.add(
        "loop-stepping",
        len(outputs) >= 3,
        f"{len(outputs)} distinct PID outputs across the soak"
        + (
            ""
            if len(outputs) >= 3
            else " -- flat output on a settled plant is fine; only investigate "
            "if convergence also failed"
        ),
        soft=True,
    )


def analyze_failsafe(samples: list[dict], checks: Checks) -> None:
    hist = zone_histogram(samples)
    emergency = sum(1 for s in samples if s["failsafe"] == "emergency")
    read_fail = sum(1 for s in samples if s["failsafe"] == "read_failures")
    checks.add(
        "zones-emergency",
        emergency == 0,
        f"{emergency} emergency samples ({hist})",
        soft=True,
    )
    checks.add(
        "zones-read-failures",
        read_fail <= len(samples) * 0.25,
        f"{read_fail} read-failure samples ({hist})",
    )


def analyze_convergence(
    samples: list[dict],
    target: float,
    tolerance: float,
    settle: float,
    band_fraction: float,
    checks: Checks,
) -> None:
    hot = [s for s in samples if s["t"] >= settle]
    frac, first = convergence(hot, target, tolerance)
    detail = (
        f"in-band {frac:.0%} (need >={band_fraction:.0%}, tol +/-{tolerance} degC) "
        f"after {settle:.0f}s settle; first in-band at "
        f"{f'{first:.1f}s' if first is not None else 'never'}; {temp_stats(hot)}"
    )
    checks.add("convergence", frac >= band_fraction and hot, detail)


def wait_port_free(srv: Srv, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state, _ = probe(srv)
        if state == "absent":
            return True
        time.sleep(0.5)
    return False


def spawn_srv(
    exe: str, port: int, target: float, config: str | None
) -> subprocess.Popen:
    argv = [exe, "--foreground", "--port", str(port), "--target-c", str(target)]
    if config:
        argv += ["--config", config]
    return subprocess.Popen(
        argv,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
    )


def engage(srv: Srv, target: float, gpu_index: int, checks: Checks) -> None:
    srv.post(f"/pid?target_c={target}")  # setpoint first...
    srv.post("/mode?value=pid")  # ...then the mode flip (optimizer order)
    time.sleep(1.5)
    st = srv.status()
    gpu = gpu_of(st, gpu_index)
    ok = st.get("mode") == "pid" and st.get("target_c") == target and gpu is not None
    checks.add(
        "engage",
        ok,
        f"mode={st.get('mode')} target={st.get('target_c')} "
        f"gpu[{gpu_index}]={'present' if gpu else 'missing'}",
    )


def teardown_owned(
    srv: Srv, spawned: bool, proc: subprocess.Popen | None, checks: Checks
) -> None:
    try:
        srv.post("/restore")
    except (SrvHttpError, OSError) as e:
        checks.add("teardown-restore", False, f"/restore failed: {e}")
    time.sleep(2.0)
    st = safe_status(srv)
    if st is None:
        checks.add("teardown-restore", True, "srv already gone after /restore")
    else:
        checks.add(
            "teardown-restore",
            st.get("mode") == "auto",
            f"mode after /restore = {st.get('mode')}",
        )
    if not spawned:
        print("  (adopted srv left running, as the optimizer does)")
        return
    try:
        srv.post("/shutdown")
    except (SrvHttpError, OSError):
        pass  # srv may already be gone -- the port check below decides
    if wait_port_free(srv, TEARDOWN_TIMEOUT_S):
        checks.add("teardown-shutdown", True, "port free after /shutdown")
    else:
        if proc is not None:
            kill_tree(proc)
        checks.add(
            "teardown-shutdown",
            False,
            "srv still listening after /shutdown -- killed hard; INVESTIGATE",
        )


def run_direct(args: argparse.Namespace, checks: Checks) -> int:
    srv = Srv(args.srv_port, args.auth)
    state, detail = probe(srv)
    proc: subprocess.Popen | None = None
    load: subprocess.Popen | None = None
    spawned = False
    if state == "occupied":
        raise EnvError(f"port {args.srv_port} occupied: {detail}")
    if state in ("ready", "starting"):
        print(f"discovery: adopting existing nvoc-srv ({detail})")
    else:
        exe = find_srv_exe(args.srv_exe)
        proc = spawn_srv(exe, args.srv_port, args.target_c, args.srv_config)
        spawned = True
        print(
            f"discovery: spawned {exe} --foreground --port {args.srv_port} "
            f"(pid {proc.pid})"
        )
    try:
        deadline = time.monotonic() + READY_TIMEOUT_S
        while True:
            state, detail = probe(srv)
            if state == "ready":
                break
            if state == "occupied":
                raise EnvError(f"srv went unhealthy during readiness wait: {detail}")
            if time.monotonic() >= deadline:
                raise EnvError(
                    f"nvoc-srv not ready in {READY_TIMEOUT_S:.0f}s ({detail})"
                )
            time.sleep(0.5)
        print(f"ready: control loop live (target {args.target_c}  degC)")
        engage(srv, args.target_c, args.gpu, checks)

        if args.load_cmd:
            load = subprocess.Popen(
                shlex.split(args.load_cmd),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            print(f"load: {args.load_cmd} (pid {load.pid})")

        soak_start = time.monotonic()
        samples = monitor(
            srv,
            args.gpu,
            stop=lambda: (time.monotonic() - soak_start) >= args.soak_seconds,
            poll=POLL_S,
        )
        if load is not None and load.poll() is not None:
            checks.add(
                "load-survived-soak",
                False,
                f"load exited early (rc={load.returncode}) -- "
                "give --load-cmd a --duration >= the soak",
            )
        else:
            checks.add("load-survived-soak", True, "load ran through the soak")
        analyze_convergence(
            samples,
            args.target_c,
            args.tolerance,
            args.settle_seconds,
            args.band_fraction,
            checks,
        )
        analyze_loop_alive(samples, checks)
        analyze_failsafe(samples, checks)
        print(f"soak: {len(samples)} samples, {zone_histogram(samples)}")

        if load is not None and not args.skip_idle_check:
            kill_tree(load)
            print(
                f"idle-check: load stopped, waiting up to "
                f"{args.idle_timeout:.0f}s for the temperature to fall below target"
            )
            fell = None
            idle_hold = False
            deadline = time.monotonic() + args.idle_timeout
            while time.monotonic() < deadline:
                st = safe_status(srv)
                gpu = gpu_of(st, args.gpu) if st is not None else None
                if gpu is not None:
                    temp = gpu.get("temp_c")
                    if temp is not None and temp < args.target_c - 0.5:
                        fell = args.idle_timeout - (deadline - time.monotonic())
                        idle_hold = (
                            gpu.get("failsafe") == "idle_hold"
                            or (gpu.get("fan_written_percent") or 100) <= 10
                        )
                        break
                time.sleep(POLL_S)
            checks.add(
                "idle-release",
                fell is not None,
                f"temp below target at {f'{fell:.0f}s' if fell is not None else 'never'}"
                + (f"; idle zone engaged: {idle_hold}" if fell is not None else ""),
            )
        teardown_owned(srv, spawned, proc, checks)
    finally:
        if proc is not None and proc.poll() is None:
            try:
                srv.post("/shutdown")
            except Exception:  # noqa: BLE001 -- best-effort cleanup
                pass
            if not wait_port_free(srv, TEARDOWN_TIMEOUT_S):
                kill_tree(proc)
        if load is not None and load.poll() is None:
            kill_tree(load)
    return EXIT_FAIL if checks.failed else EXIT_PASS


def run_via_cmd(args: argparse.Namespace, checks: Checks) -> int:
    srv = Srv(args.srv_port, args.auth)
    state, detail = probe(srv)
    if state != "absent":
        raise EnvError(
            f"port {args.srv_port} is {state} ({detail}) -- --via-cmd needs the port "
            "free; the wrapper spawns and owns the srv"
        )
    out_file = tempfile.TemporaryFile()
    err_file = tempfile.TemporaryFile()
    cmd = subprocess.Popen(
        shlex.split(args.via_cmd),
        stdout=out_file,
        stderr=err_file,
        stdin=subprocess.DEVNULL,
    )
    print(f"wrapper: {args.via_cmd} (pid {cmd.pid})")

    engaged_at = None
    start = time.monotonic()
    deadline = start + READY_TIMEOUT_S
    while cmd.poll() is None:
        st = safe_status(srv)
        if st is None:
            if time.monotonic() >= deadline:
                raise EnvError("wrapper never engaged the session (status unreachable)")
            time.sleep(0.5)
            continue
        if st.get("mode") == "pid":
            engaged_at = time.monotonic() - start
            if (
                args.expect_target is not None
                and st.get("target_c") != args.expect_target
            ):
                checks.add(
                    "engage",
                    False,
                    f"wrapper set target {st.get('target_c')} "
                    f"(expected {args.expect_target})",
                )
            else:
                checks.add(
                    "engage",
                    True,
                    f"wrapper engaged PID at t+{engaged_at:.1f}s "
                    f"(target {st.get('target_c')})",
                )
            break
        if time.monotonic() >= deadline:
            break
        time.sleep(0.5)
    if engaged_at is None:
        checks.add(
            "engage",
            False,
            f"wrapper never switched mode to pid within {READY_TIMEOUT_S:.0f}s",
        )

    samples = monitor(srv, args.gpu, stop=lambda: cmd.poll() is not None, poll=POLL_S)
    rc = cmd.wait()
    if engaged_at is not None:
        analyze_convergence(
            samples,
            args.expect_target or args.target_c,
            args.tolerance,
            args.settle_seconds,
            args.band_fraction,
            checks,
        )
        analyze_loop_alive(samples, checks)
        analyze_failsafe(samples, checks)
        print(f"session: {len(samples)} samples, wrapper exited rc={rc}")
    else:
        checks.add("session", False, f"wrapper exited rc={rc} without a live session")

    # Teardown: the wrapper spawned the srv, so it must restore AND stop it.
    time.sleep(2.0)
    st = safe_status(srv)
    if st is None:
        checks.add(
            "teardown-restore",
            True,
            "srv already gone after wrapper exit (restored + shut down)",
        )
        checks.add("teardown-shutdown", True, "port freed after wrapper exit")
    else:
        checks.add(
            "teardown-restore",
            st.get("mode") == "auto",
            f"mode after wrapper exit = {st.get('mode')} "
            "(expected auto -- wrapper must /restore)",
        )
        freed = wait_port_free(srv, TEARDOWN_TIMEOUT_S)
        checks.add(
            "teardown-shutdown",
            freed,
            "port freed after wrapper exit"
            if freed
            else "srv STILL LISTENING after wrapper exit -- leaked session",
        )

    out_file.seek(0)
    err_file.seek(0)
    tail = err_file.read().decode(errors="replace").strip().splitlines()[-5:]
    if tail:
        print("wrapper stderr tail:")
        for line in tail:
            print(f"  {line}")
    return EXIT_FAIL if checks.failed else EXIT_PASS


def main() -> int:
    ap = argparse.ArgumentParser(
        description="target-temp closed-loop smoke test (nvoc-srv control plane)"
    )
    ap.add_argument("--srv-port", type=int, default=14514)
    ap.add_argument(
        "--auth", metavar="USER:PASS", help="HTTP Basic credentials (srv auth=auto/on)"
    )
    ap.add_argument("--gpu", type=int, default=0, help="GPU index to monitor")
    ap.add_argument("--target-c", type=float, default=70.0)
    ap.add_argument(
        "--soak-seconds",
        type=float,
        default=120.0,
        help="direct mode: monitored duration under load",
    )
    ap.add_argument(
        "--settle-seconds",
        type=float,
        default=45.0,
        help="samples before this point are excluded from the convergence check",
    )
    ap.add_argument(
        "--tolerance",
        type=float,
        default=3.0,
        help="in-band = |temp − target| <= tolerance ( degC)",
    )
    ap.add_argument(
        "--band-fraction",
        type=float,
        default=0.8,
        help="required in-band fraction of post-settle samples",
    )
    ap.add_argument(
        "--idle-timeout",
        type=float,
        default=120.0,
        help="direct mode: how long to wait for the temperature to "
        "fall below target after the load stops",
    )
    ap.add_argument("--skip-idle-check", action="store_true")
    ap.add_argument(
        "--load-cmd",
        help="direct mode: load command, e.g. "
        "'cli-stressor-cuda-rs --profile standard --duration 150'",
    )
    ap.add_argument(
        "--srv-exe",
        help="direct mode: nvoc-srv binary (default: PATH / target/release)",
    )
    ap.add_argument(
        "--srv-config",
        help="direct mode: TOML passed to the spawned srv via --config "
        '(e.g. a scratch file with auth = "off")',
    )
    ap.add_argument(
        "--via-cmd",
        metavar="CMD",
        help="wrapper mode: run CMD (optimizer / stressor with "
        "--target-temp) and verify ITS session lifecycle",
    )
    ap.add_argument(
        "--expect-target",
        type=float,
        help="via-cmd mode: assert the wrapper's setpoint",
    )
    args = ap.parse_args()

    checks = Checks()
    print("== target-temp smoke ==")
    try:
        if args.via_cmd:
            code = run_via_cmd(args, checks)
        else:
            code = run_direct(args, checks)
    except EnvError as e:
        print(f"ENV ERROR: {e}")
        return EXIT_ENV
    checks.report()
    print(f"\nresult: {'PASS' if code == EXIT_PASS else 'FAIL'}")
    return code


if __name__ == "__main__":
    sys.exit(main())
