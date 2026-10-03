"""target-temp smoke test for the **resident** nvoc-srv control plane.

Exercises the consumer/session plane end to end -- the part that replaced the
old "the optimizer spawns and owns a srv child" model:

* discovery (adopt a resident srv, or spawn a foreground one for the test),
* registration (``POST /api/session/open``), a control claim
  (``POST /api/session/claim?mode=pid&target_c=``), and a live heartbeat,
* closed-loop convergence under load,
* idle release,
* arbitration: a lower-priority claim does not preempt the owner; closing the
  owner hands control to the next claim, not straight to Auto,
* crash-safety: a hard-killed higher-priority consumer's lease lapses and
  control falls back to the next claim,
* teardown: closing the last claim restores ``auto``.

Everything talks to the documented HTTP API; no GPU code is invoked from this
script. The srv is treated as resident and shared -- this script never asserts
that it owns the srv process.

Modes:

* **direct** (default) -- the script registers its own consumer and drives the
  closed loop::

      python srv/test/target_temp_smoke.py \
          --load-cmd "cli-stressor-cuda-rs --profile standard --duration 180" \
          --target-c 70

  If no srv is listening, pass ``--srv-exe`` to spawn a foreground one for the
  duration of the test (a dev convenience; a production box runs the installed
  service).

* **--via-cmd** -- run a wrapper (the optimizer's ``--target-temp`` scan) that
  registers *itself*, and verify ITS session lifecycle end to end::

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
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

CSRF = {"X-Requested-With": "XMLHttpRequest"}
EXIT_PASS, EXIT_FAIL, EXIT_ENV = 0, 1, 2

READY_TIMEOUT_S = 15.0
POLL_S = 1.0
IO_TIMEOUT_S = 5.0
TEARDOWN_TIMEOUT_S = 15.0
HEARTBEAT_S = 10.0

# Mirror the client crate's priority bands (srv-client/src/lib.rs).
PRIORITY_STRESSOR = 10
PRIORITY_OPTIMIZER = 20
# The srv lease window (srv/src/session.rs LEASE_TTL); a lapsed crash-safety
# check must wait at least this long plus a sweep tick.
LEASE_TTL_S = 30.0


class EnvError(Exception):
    """Environment problem (port, spawn, auth) -- not a controller failure."""


class SrvHttpError(Exception):
    def __init__(self, code: int, body: str) -> None:
        super().__init__(f"HTTP {code}: {body.strip()[:200]}")
        self.code = code


class Srv:
    """Minimal client for the loopback control plane (mirrors srv-client)."""

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

    def _auth_hint(self) -> EnvError:
        return EnvError(
            "srv requires authentication (auth=auto/on) -- pass --auth USER:PASS "
            'or set auth = "off" in the srv config'
        )

    def status(self) -> dict:
        """GET /status as JSON. Raises EnvError on 401 (auth hint)."""
        code, body = self._request("GET", "/status")
        if code == 401:
            raise self._auth_hint()
        if code != 200:
            raise SrvHttpError(code, body)
        try:
            return json.loads(body)
        except json.JSONDecodeError as e:
            raise SrvHttpError(code, f"/status is not JSON: {e}") from e

    def post(self, path: str) -> str:
        return self.post_json(path)  # noqa: RET504 -- kept for call-site clarity

    def post_json(self, path: str) -> str:
        code, body = self._request("POST", path)
        if code == 401:
            raise self._auth_hint()
        if code != 200:
            raise SrvHttpError(code, body)
        return body

    # ---- consumer/session plane ----

    def session_open(self, name: str, priority: int) -> dict:
        q = f"/api/session/open?name={urllib.parse.quote(name)}&priority={priority}"
        return json.loads(self.post_json(q))

    def session_claim(
        self,
        sid: str,
        mode: str,
        target_c: float | None = None,
        manual_percent: int | None = None,
    ) -> dict:
        q = f"/api/session/claim?id={urllib.parse.quote(sid)}&mode={mode}"
        if target_c is not None:
            q += f"&target_c={target_c}"
        if manual_percent is not None:
            q += f"&manual_percent={manual_percent}"
        return json.loads(self.post_json(q))

    def session_heartbeat(self, sid: str) -> dict:
        return json.loads(
            self.post_json(f"/api/session/heartbeat?id={urllib.parse.quote(sid)}")
        )

    def session_close(self, sid: str) -> None:
        self.post_json(f"/api/session/close?id={urllib.parse.quote(sid)}")

    def sessions(self) -> dict:
        code, body = self._request("GET", "/api/sessions")
        if code == 401:
            raise self._auth_hint()
        if code != 200:
            raise SrvHttpError(code, body)
        return json.loads(body)


class HeldSession:
    """A registered consumer with a background heartbeat, like the real client.

    Dropping/calling :meth:`close` deregisters. Leaving it alive without
    heartbeating (see the hold-session helper) simulates a crash whose lease
    lapses on the srv side.
    """

    def __init__(
        self, srv: Srv, name: str, priority: int, *, beat: bool = True
    ) -> None:
        opened = srv.session_open(name, priority)
        self.srv = srv
        self.id = opened["id"]
        self.name = name
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        if beat:
            self._thread = threading.Thread(
                target=self._beat_loop, name="smoke-heartbeat", daemon=True
            )
            self._thread.start()

    def _beat_loop(self) -> None:
        while not self._stop.wait(HEARTBEAT_S):
            try:
                self.srv.session_heartbeat(self.id)
            except (SrvHttpError, OSError):
                pass

    def claim(
        self,
        mode: str,
        target_c: float | None = None,
        manual_percent: int | None = None,
    ) -> bool:
        return bool(
            self.srv.session_claim(
                self.id, mode, target_c=target_c, manual_percent=manual_percent
            ).get("owner")
        )

    def close(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=HEARTBEAT_S + IO_TIMEOUT_S)
            self._thread = None
        try:
            self.srv.session_close(self.id)
        except (SrvHttpError, OSError):
            pass


def probe(srv: Srv) -> tuple[str, str]:
    """Tri-state probe, same semantics as srv-client/src/http.rs:
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


def find_srv_exe(explicit: str) -> str:
    if os.path.exists(explicit):
        return explicit
    raise EnvError(f"--srv-exe not found: {explicit}")


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
    """/status tolerating the srv vanishing mid-poll. EnvError (auth) still
    propagates -- it must surface."""
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
                "err": None,
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
            "err": gpu.get("last_error"),
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


def actuator_dead(samples: list[dict]) -> str | None:
    """Detect a backend that cannot drive the fan at all (e.g. the NVAPI NDA
    fan surface is rejected and the NVML fallback is unavailable -- on Windows
    nvml.dll exports no fan-write symbols, so `backend.rs` treats that path as
    dead by construction). Returns the sample error string when a majority of
    engaged samples report a write failure, else None.

    Convergence is unattainable without an actuator; attributing that to the
    hardware keeps the check honest instead of blaming the control plane."""
    engaged = [s for s in samples if s["mode"] == "pid"]
    if not engaged:
        return None
    errs = [s["err"] for s in engaged if s["err"]]
    if len(errs) > len(engaged) * 0.5:
        return errs[0]
    return None


def analyze_convergence(
    samples: list[dict],
    target: float,
    tolerance: float,
    settle: float,
    band_fraction: float,
    checks: Checks,
    dead: str | None = None,
) -> None:
    hot = [s for s in samples if s["t"] >= settle]
    frac, first = convergence(hot, target, tolerance)
    detail = (
        f"in-band {frac:.0%} (need >={band_fraction:.0%}, tol +/-{tolerance} degC) "
        f"after {settle:.0f}s settle; first in-band at "
        f"{f'{first:.1f}s' if first is not None else 'never'}; {temp_stats(hot)}"
    )
    if dead is not None:
        detail += (
            f" -- fan writes failing ({dead}), so the actuator is unavailable "
            "on this box; treating convergence as a warning"
        )
    checks.add(
        "convergence", frac >= band_fraction and hot, detail, soft=dead is not None
    )


def wait_port_free(srv: Srv, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        state, _ = probe(srv)
        if state == "absent":
            return True
        time.sleep(0.5)
    return False


def spawn_srv(exe: str, port: int, config: str | None) -> subprocess.Popen:
    argv = [exe, "--foreground", "--port", str(port)]
    if config:
        argv += ["--config", config]
    return subprocess.Popen(
        argv,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
    )


def ensure_srv(
    args: argparse.Namespace, checks: Checks
) -> tuple[Srv, subprocess.Popen | None]:
    """Adopt a resident srv, or spawn a foreground one when --srv-exe is given.
    Returns (client, spawned_proc_or_None). Raises EnvError when neither works."""
    srv = Srv(args.srv_port, args.auth)
    state, detail = probe(srv)
    proc: subprocess.Popen | None = None
    if state == "occupied":
        raise EnvError(f"port {args.srv_port} occupied: {detail}")
    if state in ("ready", "starting"):
        print(f"discovery: adopting resident nvoc-srv ({detail})")
    else:
        if not args.srv_exe:
            raise EnvError(
                f"no nvoc-srv listening on port {args.srv_port} ({detail}) -- start "
                "the resident service, or pass --srv-exe to spawn a foreground srv "
                "for this test"
            )
        exe = find_srv_exe(args.srv_exe)
        proc = spawn_srv(exe, args.srv_port, args.srv_config)
        print(
            f"discovery: spawned {exe} --foreground --port {args.srv_port} "
            f"(pid {proc.pid})"
        )

    deadline = time.monotonic() + READY_TIMEOUT_S
    while True:
        state, detail = probe(srv)
        if state == "ready":
            break
        if state == "occupied":
            raise EnvError(f"srv went unhealthy during readiness wait: {detail}")
        if time.monotonic() >= deadline:
            raise EnvError(f"nvoc-srv not ready in {READY_TIMEOUT_S:.0f}s ({detail})")
        time.sleep(0.5)
    print("ready: control loop live")
    return srv, proc


def wait_mode(srv: Srv, want: str, timeout: float) -> tuple[bool, str]:
    """Wait for the top-level config mode to become `want`."""
    deadline = time.monotonic() + timeout
    last = "?"
    while time.monotonic() < deadline:
        st = safe_status(srv)
        if st is not None:
            last = str(st.get("mode"))
            if last == want:
                return True, last
        time.sleep(0.5)
    return False, last


def check_arbitration(
    srv: Srv,
    owner: HeldSession,
    gpu_index: int,
    args: argparse.Namespace,
    checks: Checks,
) -> None:
    """No-preempt: a lower-priority claim must not take control from the owner."""
    low = HeldSession(srv, "smoke-low", max(args.priority - 10, -1000))
    try:
        low_owner = low.claim("manual", manual_percent=60)
        st = safe_status(srv)
        mode = st.get("mode") if st else "?"
        checks.add(
            "arbitration-no-preempt",
            (not low_owner) and mode == "pid",
            f"low-priority claim owner={low_owner}, mode={mode} (expected the "
            "PID owner to keep control)",
        )
        # Closing a non-owner must not disturb the owner.
        low.close()
        st = safe_status(srv)
        mode = st.get("mode") if st else "?"
        checks.add(
            "arbitration-stable",
            mode == "pid",
            f"mode after closing the non-owner = {mode}",
        )
    finally:
        low.close()


def check_crash_safety(
    args: argparse.Namespace, checks: Checks, srv: Srv, owner: HeldSession
) -> None:
    """A hard-killed higher-priority consumer's lease must lapse, returning
    control to the next claim."""
    helper = subprocess.Popen(
        [
            sys.executable,
            os.path.abspath(__file__),
            "--srv-port",
            str(args.srv_port),
            "--gpu",
            str(args.gpu),
            "--hold-session",
            "--hold-priority",
            str(args.priority + 10),
            "--target-c",
            str(args.target_c),
        ]
        + (["--auth", args.auth] if args.auth else []),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        stdin=subprocess.DEVNULL,
    )
    try:
        preempted, mode = wait_mode(srv, "manual", 15.0)
        checks.add(
            "crash-preempt",
            preempted,
            f"higher-priority holder took control: mode={mode}",
        )
        kill_tree(helper)
        print(
            f"crash-safety: holder killed, waiting up to "
            f"{args.lease_timeout:.0f}s for the lease to lapse"
        )
        # The lapsed holder falls back to the still-live owner (pid), not Auto.
        restored, mode = wait_mode(srv, "pid", args.lease_timeout)
        checks.add(
            "crash-lease-lapse",
            restored,
            f"control returned to the next claim after the crash: mode={mode}",
        )
    finally:
        kill_tree(helper)


def check_idle_release(
    args: argparse.Namespace, checks: Checks, srv: Srv, load: subprocess.Popen | None
) -> None:
    if load is None or args.skip_idle_check:
        return
    kill_tree(load)
    print(
        f"idle-check: load stopped, waiting up to {args.idle_timeout:.0f}s for the "
        "temperature to fall below target"
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


def run_direct(args: argparse.Namespace, checks: Checks) -> int:
    srv, proc = ensure_srv(args, checks)
    owner = HeldSession(srv, "smoke-direct", args.priority)
    load: subprocess.Popen | None = None
    try:
        if not owner.claim("pid", target_c=args.target_c):
            checks.add("engage", False, "claim reported we are not the owner")
            return EXIT_FAIL
        engaged, mode = wait_mode(srv, "pid", 5.0)
        st = safe_status(srv)
        target = st.get("target_c") if st else None
        gpu = gpu_of(st, args.gpu) if st else None
        checks.add(
            "engage",
            engaged and target == args.target_c and gpu is not None,
            f"mode={mode} target={target} "
            f"gpu[{args.gpu}]={'present' if gpu else 'missing'}",
        )

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
                f"load exited early (rc={load.returncode}) -- give --load-cmd a "
                "--duration >= the soak",
            )
        elif load is not None:
            checks.add("load-survived-soak", True, "load ran through the soak")
        analyze_convergence(
            samples,
            args.target_c,
            args.tolerance,
            args.settle_seconds,
            args.band_fraction,
            checks,
            dead=actuator_dead(samples),
        )
        analyze_loop_alive(samples, checks)
        analyze_failsafe(samples, checks)
        print(f"soak: {len(samples)} samples, {zone_histogram(samples)}")

        check_idle_release(args, checks, srv, load)
        load = None  # stopped inside the check

        check_arbitration(srv, owner, args.gpu, args, checks)
        if not args.skip_lease_check:
            check_crash_safety(args, checks, srv, owner)

        # Closing the last claim restores Auto (arbitration hand-back).
        owner.close()
        restored, mode = wait_mode(srv, "auto", 15.0)
        checks.add(
            "teardown-restore",
            restored,
            f"mode after closing the last claim = {mode}",
        )
        left = srv.sessions()
        checks.add(
            "teardown-session-gone",
            not any(s["id"] == owner.id for s in left.get("sessions", [])),
            f"{len(left.get('sessions', []))} session(s) still registered",
        )
    finally:
        owner.close()
        if load is not None and load.poll() is None:
            kill_tree(load)
        if proc is not None and proc.poll() is None:
            try:
                srv.post("/shutdown")
            except Exception:  # noqa: BLE001 -- best-effort dev cleanup
                pass
            if not wait_port_free(srv, TEARDOWN_TIMEOUT_S):
                kill_tree(proc)
    return EXIT_FAIL if checks.failed else EXIT_PASS


def run_via_cmd(args: argparse.Namespace, checks: Checks) -> int:
    srv, proc = ensure_srv(args, checks)
    try:
        cmd = subprocess.Popen(
            shlex.split(args.via_cmd),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            stdin=subprocess.DEVNULL,
        )
        print(f"wrapper: {args.via_cmd} (pid {cmd.pid})")

        # Wait for the wrapper's consumer to register and take control.
        registered, owned_at, saw_owner = None, None, False
        start = time.monotonic()
        deadline = start + READY_TIMEOUT_S
        while cmd.poll() is None and time.monotonic() < deadline:
            st = safe_status(srv)
            if st is not None and st.get("mode") == "pid":
                sessions = srv.sessions()
                owner_id = sessions.get("owner")
                consumer = next(
                    (s for s in sessions.get("sessions", []) if s["id"] == owner_id),
                    None,
                )
                if consumer is not None:
                    registered = consumer["name"]
                    saw_owner = True
                    owned_at = time.monotonic() - start
                    target = st.get("target_c")
                    if args.expect_target is not None and target != args.expect_target:
                        checks.add(
                            "register",
                            False,
                            f"wrapper set target {target} "
                            f"(expected {args.expect_target})",
                        )
                    else:
                        checks.add(
                            "register",
                            True,
                            f"consumer {registered!r} owns control at "
                            f"t+{owned_at:.1f}s (target {target})",
                        )
                    break
            time.sleep(0.5)
        if not saw_owner:
            checks.add("register", False, "wrapper never registered an owning session")

        rc = cmd.wait()
        print(f"session: wrapper exited rc={rc}")

        # The wrapper must release control on its cleanup path; allow up to the
        # lease window in case it was killed hard rather than exiting cleanly.
        restored, mode = wait_mode(srv, "auto", args.lease_timeout)
        checks.add(
            "teardown-restore",
            restored,
            f"mode after wrapper exit = {mode}",
        )
        sessions = srv.sessions()
        leftover = [s["name"] for s in sessions.get("sessions", [])]
        checks.add(
            "teardown-session-gone",
            not leftover,
            f"remaining sessions: {leftover or 'none'}",
        )

        if cmd.stderr is not None:
            tail = cmd.stderr.read().decode(errors="replace").strip().splitlines()[-5:]
            if tail:
                print("wrapper stderr tail:")
                for line in tail:
                    print(f"  {line}")
    finally:
        if proc is not None and proc.poll() is None:
            try:
                srv.post("/shutdown")
            except Exception:  # noqa: BLE001 -- best-effort dev cleanup
                pass
            if not wait_port_free(srv, TEARDOWN_TIMEOUT_S):
                kill_tree(proc)
    return EXIT_FAIL if checks.failed else EXIT_PASS


def hold_session(args: argparse.Namespace) -> int:
    """Crash-safety helper: register + claim, then sleep WITHOUT heartbeating,
    so the parent can hard-kill this process and watch the lease lapse."""
    srv = Srv(args.srv_port, args.auth)
    # Beat a few times so the session is unmistakably live, then stop.
    holder = HeldSession(srv, "smoke-hold", args.hold_priority)
    holder.claim("manual", manual_percent=75)
    try:
        while True:
            time.sleep(1.0)
    except KeyboardInterrupt:
        return 0
    finally:
        holder.close()
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(
        description="target-temp closed-loop smoke test (resident nvoc-srv)"
    )
    ap.add_argument("--srv-port", type=int, default=14514)
    ap.add_argument(
        "--auth", metavar="USER:PASS", help="HTTP Basic credentials (srv auth=auto/on)"
    )
    ap.add_argument("--gpu", type=int, default=0, help="GPU index to monitor")
    ap.add_argument("--target-c", type=float, default=70.0)
    ap.add_argument(
        "--priority",
        type=int,
        default=PRIORITY_OPTIMIZER,
        help="priority band for the direct-mode consumer (default: optimizer=20)",
    )
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
        help="in-band = |temp - target| <= tolerance (degC)",
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
        help="direct mode: how long to wait for the temperature to fall below "
        "target after the load stops",
    )
    ap.add_argument("--skip-idle-check", action="store_true")
    ap.add_argument(
        "--skip-lease-check",
        action="store_true",
        help="direct mode: skip the hard-kill / lease-lapse crash-safety check",
    )
    ap.add_argument(
        "--lease-timeout",
        type=float,
        default=LEASE_TTL_S + 30.0,
        help="how long to wait for a lapsed lease to be swept",
    )
    ap.add_argument(
        "--load-cmd",
        help="direct mode: load command, e.g. "
        "'cli-stressor-cuda-rs --profile standard --duration 180'",
    )
    ap.add_argument(
        "--srv-exe",
        help="dev only: spawn this nvoc-srv --foreground when nothing is "
        "listening (default: require a resident srv)",
    )
    ap.add_argument(
        "--srv-config",
        help="dev only: TOML passed to the spawned srv via --config "
        '(e.g. a scratch file with auth = "off")',
    )
    ap.add_argument(
        "--via-cmd",
        metavar="CMD",
        help="wrapper mode: run CMD (optimizer --target-temp) and verify ITS "
        "registration/restore lifecycle",
    )
    ap.add_argument(
        "--expect-target",
        type=float,
        help="via-cmd mode: assert the wrapper's setpoint",
    )
    # Crash-safety helper (internal): register, claim, then sleep forever.
    ap.add_argument("--hold-session", action="store_true", help=argparse.SUPPRESS)
    ap.add_argument(
        "--hold-priority",
        type=int,
        default=PRIORITY_OPTIMIZER + 10,
        help=argparse.SUPPRESS,
    )
    args = ap.parse_args()

    if args.hold_session:
        return hold_session(args)

    checks = Checks()
    print("== target-temp smoke (resident srv) ==")
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
