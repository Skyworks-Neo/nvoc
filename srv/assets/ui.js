/* NVOC-SRV console — vanilla JS, polls /status, posts mutations with the
   CSRF header. No dependencies; served from the loopback control plane. */
"use strict";

const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];

const CSRF = { "X-Requested-With": "XMLHttpRequest" };
const UNIT = { fan_temp: "°C", freq_temp: "°C", freq_power: "W" };

let cfg = null;
let lastOk = null;

function toast(msg, bad = false) {
  let el = $(".toast");
  if (!el) {
    el = document.createElement("div");
    el.className = "toast";
    document.body.appendChild(el);
  }
  el.textContent = msg;
  el.classList.toggle("bad", bad);
  el.classList.add("show");
  clearTimeout(el._t);
  el._t = setTimeout(() => el.classList.remove("show"), 2600);
}

async function post(path) {
  const res = await fetch(path, { method: "POST", headers: CSRF });
  const text = await res.text();
  toast(`${res.status} ${text}`, !res.ok);
  if (res.ok) await loadConfig();
  return res.ok;
}

async function pollStatus() {
  try {
    const res = await fetch("/status");
    if (!res.ok) throw new Error(res.status);
    render(await res.json());
    setConn("ok", "live");
    lastOk = Date.now();
  } catch (e) {
    const secs = lastOk ? Math.round((Date.now() - lastOk) / 1000) : "…";
    setConn(lastOk ? "stale" : "dead", `stale ${secs}s`);
  }
}

function setConn(cls, text) {
  const el = $("#conn");
  el.className = `conn ${cls}`;
  el.textContent = text;
}

async function loadConfig() {
  try {
    cfg = await (await fetch("/config")).json();
    fillTuning(cfg);
    markSeg("#loopseg", "loop", cfg.loop_kind);
    markSeg("#modeseg", "mode", cfg.mode);
    $("#tuneloop").textContent = cfg.loop_kind;
  } catch (e) {
    /* status poll will flag connectivity */
  }
}

function markSeg(sel, attr, value) {
  $$( `${sel} button`).forEach((b) =>
    b.classList.toggle("on", b.dataset[attr] === value)
  );
}

/* ---------- per-GPU cards ---------- */

function gpuCard(g) {
  let el = document.getElementById(`gpu-${g.index}`);
  if (!el) {
    el = document.createElement("div");
    el.className = "gpu";
    el.id = `gpu-${g.index}`;
    el.innerHTML = `
      <div class="gpu-head">
        <span class="name"></span>
        <span class="badge"></span>
        <span class="gmode"></span>
      </div>
      <div class="gauges">
        <div class="gauge" data-g="core"><span class="val mono"></span><span class="lbl">core</span></div>
        <div class="gauge" data-g="hot"><span class="val mono"></span><span class="lbl">hotspot</span></div>
        <div class="gauge" data-g="fan"><span class="val mono"></span><span class="lbl">fan</span></div>
      </div>
      <div class="metrics">
        <span class="k">loop value</span><span class="v mono" data-m="loop"></span>
        <span class="k">fan written / tach</span><span class="v mono" data-m="fan"></span>
        <span class="k">freq cap / clock</span><span class="v mono" data-m="cap"></span>
        <span class="k">power</span><span class="v mono" data-m="power"></span>
      </div>
      <table class="pterms">
        <tr><th>error</th><th>p</th><th>i</th><th>d</th><th>base</th><th>out</th></tr>
        <tr>
          <td data-t="error_c"></td><td data-t="p"></td><td data-t="i"></td>
          <td data-t="d"></td><td data-t="base_percent"></td><td data-t="output_percent"></td>
        </tr>
        <tr><th colspan="3" style="text-align:left">gains in effect</th>
            <td colspan="3" data-t="gains" style="text-align:left"></td></tr>
      </table>
      <div class="err"></div>`;
    $("#gpus").appendChild(el);
  }
  return el;
}

const tempPct = (t) => Math.max(0, Math.min(100, ((t - 30) / 70) * 100));

function setGauge(el, pct, color, text) {
  el.style.setProperty("--p", pct);
  el.style.setProperty("--gc", color);
  el.querySelector(".val").textContent = text;
}

function tempColor(t, target) {
  if (target == null) return "var(--accent)";
  const d = t - target;
  if (d > 8) return "var(--bad)";
  if (d > 2) return "var(--warn)";
  if (d < -10) return "var(--blue)";
  return "var(--good)";
}

function render(s) {
  $("#summary").innerHTML =
    `loop <b>${s.mode}</b> · target <b class="mono">${fmt(s.target_c, 1)}</b>` +
    ` · <b class="mono">${s.interval_ms}</b> ms`;

  for (const g of s.gpus) {
    const el = gpuCard(g);
    el.querySelector(".name").textContent = g.name;
    const badge = el.querySelector(".badge");
    badge.textContent = g.failsafe;
    badge.className = `badge ${g.failsafe}`;
    el.querySelector(".gmode").textContent = g.mode;

    const core = g.sensors?.core_c ?? g.temp_c;
    const hot = g.sensors?.hotspot_c;
    setGauge(el.querySelector('[data-g="core"]'), core == null ? 0 : tempPct(core),
      tempColor(core, s.target_c), core == null ? "—" : core.toFixed(1));
    setGauge(el.querySelector('[data-g="hot"]'), hot == null ? 0 : tempPct(hot),
      hot == null ? "#1c2230" : "var(--accent2)", hot == null ? "—" : hot.toFixed(1));

    const fan = g.fan_measured_percent ?? g.fan_written_percent ?? 0;
    setGauge(el.querySelector('[data-g="fan"]'), fan, "var(--good)", `${fan}%`);

    const loopVal = g.power_w ?? g.temp_c;
    el.querySelector('[data-m="loop"]').textContent =
      loopVal == null ? "—" : loopVal.toFixed(1) + (s.loop_kind === "freq_power" ? " W" : " °C");
    el.querySelector('[data-m="fan"]').textContent =
      `${g.fan_written_percent ?? "—"} / ${g.fan_measured_percent ?? "—"}`;
    el.querySelector('[data-m="cap"]').textContent =
      g.cap_mhz == null ? "—" : `${g.cap_mhz} / ${g.core_clock_mhz ?? "—"} MHz`;
    el.querySelector('[data-m="power"]').textContent =
      g.power_w == null ? "—" : `${g.power_w.toFixed(1)} W`;

    const pt = el.querySelector(".pterms");
    pt.style.display = g.pid ? "" : "none";
    if (g.pid) {
      for (const k of ["error_c", "p", "i", "d", "base_percent", "output_percent"]) {
        pt.querySelector(`[data-t="${k}"]`).textContent = fmt(g.pid[k], 2);
      }
      pt.querySelector('[data-t="gains"]').textContent =
        `kp ${g.pid.kp} · ki ${g.pid.ki} · kd ${g.pid.kd}`;
    }
    el.querySelector(".err").textContent = g.last_error ?? "";
  }
}

const fmt = (v, d) => (v == null || Number.isNaN(+v) ? "—" : (+v).toFixed(d));

/* ---------- tuning form ---------- */

function fillTuning(c) {
  const f = $("#tune");
  const active = c.loop_kind === "fan_temp" ? c.pid : c.freq;
  const set = (name, v) => {
    const el = f.elements[name];
    if (!el) return;
    el.placeholder = v == null ? "" : v;
    if (!el.dataset.user) el.value = v ?? "";
  };
  set("target_c", c.loop_kind === "fan_temp" ? c.pid.target_c : c.freq.target);
  set("kp", active.kp); set("ki", active.ki); set("kd", active.kd);
  set("base_percent", active.base_percent);
  set("min_percent", active.min_percent); set("max_percent", active.max_percent);
  if (c.loop_kind === "fan_temp") {
    set("idle_delta_c", c.pid.idle_delta_c);
    set("emergency_delta_c", c.pid.emergency_delta_c);
    f.elements["temp_guard_c"].value = ""; f.elements["temp_guard_c"].placeholder = "n/a";
  } else {
    set("idle_delta_c", c.freq.idle_delta);
    set("emergency_delta_c", c.freq.emergency_delta);
    set("temp_guard_c", c.freq.temp_guard_c);
  }
  set("write_deadband_percent", c.loop_kind === "fan_temp"
    ? c.pid.write_deadband_percent : c.freq.write_deadband_percent);
  set("interval_ms", c.interval_ms);
  f.elements["sensor"].value = c.sensor;
  f.elements["adaptive_base"].value = String(active.adaptive_base ?? "");
  $("#manual").value = c.manual_percent ?? 50;
  $("#manualv").textContent = c.manual_percent ?? 50;
}

$("#tune").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const q = new URLSearchParams();
  for (const el of $("#tune").elements) {
    if (!el.name || el.value === "" || el.value === null) continue;
    if (el.value !== String(el.placeholder)) q.set(el.name, el.value);
  }
  if (![...q.keys()].length) { toast("nothing changed", true); return; }
  const ok = await post(`/pid?${q}`);
  if (ok) await loadConfig();
});

$("#reload").addEventListener("click", loadConfig);

/* keep fields the user edited from being clobbered by the config refill */
$("#tune").addEventListener("input", (ev) => { ev.target.dataset.user = "1"; });

/* ---------- control row ---------- */

$("#loopseg").addEventListener("click", async (ev) => {
  const loop = ev.target.dataset.loop;
  if (loop && (await post(`/loop?value=${loop}`))) await loadConfig();
});
$("#modeseg").addEventListener("click", async (ev) => {
  const mode = ev.target.dataset.mode;
  if (mode && (await post(`/mode?value=${mode}`))) await loadConfig();
});
$("#manual").addEventListener("input", (ev) => {
  $("#manualv").textContent = ev.target.value;
});
$("#pin").addEventListener("click", () =>
  post(`/fan?percent=${$("#manual").value}`));
$("#restore").addEventListener("click", () => post("/restore"));
$("#shutdown").addEventListener("click", () => {
  if (confirm("Shut the control service down? Fans return to the driver."))
    post("/shutdown");
});

$("#hint").textContent =
  "manual pins the fan duty (fan_temp) or the restriction effort / cap (freq loops)";

/* ---------- start ---------- */

loadConfig();
pollStatus();
setInterval(pollStatus, 1000);
setInterval(loadConfig, 10000);
