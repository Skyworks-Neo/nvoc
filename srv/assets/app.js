/* NVOC-SRV console — vanilla JS SPA (hash-routed), uPlot for time series.
   Polls /api/status + /api/history; mutations POST with the CSRF header. */
"use strict";

const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
const CSRF = { "X-Requested-With": "XMLHttpRequest" };

let cfg = null;
let lastOk = null;
let rangeSecs = 300;
let charts = {};        // key -> uPlot instance (dashboard)
let chartsReady = false;
let vfChart = null;     // uPlot instance (v/f curve)

function toast(msg, bad = false) {
  const el = $("#toast");
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

/* Confirm dialog: returns a promise resolved on Confirm. */
function confirmBox(title, text) {
  return new Promise((resolve) => {
    const box = $("#confirm");
    $("#confirm-title").textContent = title;
    $("#confirm-text").textContent = text;
    box.classList.remove("hidden");
    const done = (v) => {
      box.classList.add("hidden");
      $("#confirm-yes").removeAttribute("onclick");
      $("#confirm-no").removeAttribute("onclick");
      resolve(v);
    };
    $("#confirm-yes").onclick = () => done(true);
    $("#confirm-no").onclick = () => done(false);
  });
}

async function pollStatus() {
  try {
    const res = await fetch("/api/status");
    if (!res.ok) throw new Error(res.status);
    renderStatus(await res.json());
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

/* ---------- routing ---------- */
const PAGES = ["dashboard", "control", "overclock", "fan", "vfcurve", "about"];

function route() {
  const page = (location.hash || "#/dashboard").replace("#/", "") || "dashboard";
  $$(".page").forEach((p) => p.classList.toggle("on", p.id === `page-${page}`));
  $$(".nav a[data-page]").forEach((a) => a.classList.toggle("on", a.dataset.page === page));
  if (page === "dashboard" && !chartsReady) {
    // The page just became visible — uPlot can measure its containers now.
    initCharts();
    loadHistory();
  }
  if (page === "vfcurve") refreshVf();
  if (page === "about") refreshAbout();
  if (page === "overclock") refreshOc();
}
window.addEventListener("hashchange", route);

/* ---------- config / tuning ---------- */
async function loadConfig() {
  try {
    cfg = await (await fetch("/config")).json();
    markSeg("#loopseg", "loop", cfg.loop_kind);
    markSeg("#modeseg", "mode", cfg.mode);
    $("#tuneloop").textContent = cfg.loop_kind;
    fillTuning(cfg);
  } catch (e) { /* conn widget covers it */ }
}

function markSeg(sel, attr, value) {
  $$(`${sel} button`).forEach((b) => b.classList.toggle("on", b.dataset[attr] === value));
}

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
  ["kp", "ki", "kd", "base_percent", "min_percent", "max_percent"].forEach((k) => set(k, active[k]));
  if (c.loop_kind === "fan_temp") {
    set("idle_delta_c", c.pid.idle_delta_c);
    set("emergency_delta_c", c.pid.emergency_delta_c);
    f.elements["temp_guard_c"].value = ""; f.elements["temp_guard_c"].placeholder = "n/a";
  } else {
    set("idle_delta_c", c.freq.idle_delta);
    set("emergency_delta_c", c.freq.emergency_delta);
    set("temp_guard_c", c.freq.temp_guard_c);
  }
  set("write_deadband_percent",
    c.loop_kind === "fan_temp" ? c.pid.write_deadband_percent : c.freq.write_deadband_percent);
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
  if (await post(`/pid?${q}`)) await loadConfig();
});
$("#reload").addEventListener("click", loadConfig);
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
$("#manual").addEventListener("input", (ev) => { $("#manualv").textContent = ev.target.value; });
$("#pin").addEventListener("click", () => post(`/fan?percent=${$("#manual").value}`));
$("#restore").addEventListener("click", () => post("/restore"));
$("#shutdown").addEventListener("click", async () => {
  if (await confirmBox("Shutdown service",
    "Stop the control service? Fans return to the driver's own curve.")) {
    post("/shutdown");
  }
});

/* ---------- dashboard ---------- */
const fmt = (v, d = 1) => (v == null || Number.isNaN(+v) ? "—" : (+v).toFixed(d));

function gpuCard(g) {
  let el = document.getElementById(`gpu-${g.index}`);
  if (!el) {
    el = document.createElement("div");
    el.className = "gpu";
    el.id = `gpu-${g.index}`;
    el.innerHTML = `
      <div class="gpu-head">
        <span class="name"></span>
        <span class="badge mono"></span>
        <span class="gmode hint"></span>
      </div>
      <div class="bignums mono">
        <div><div class="v" data-k="temp">—</div><div class="u">temp °C</div></div>
        <div><div class="v" data-k="pwr">—</div><div class="u">power W</div></div>
        <div><div class="v" data-k="clk">—</div><div class="u">core MHz</div></div>
        <div><div class="v" data-k="fan">—</div><div class="u">fan %</div></div>
      </div>
      <div class="spark" data-spark></div>
      <div class="spark-legend"><span>temp (60 s)</span><span>fan %</span></div>`;
    $("#cards").appendChild(el);
    el._spark = makeSpark(el.querySelector("[data-spark]"));
  }
  return el;
}

function makeSpark(el) {
  if (!window.uPlot || el.clientWidth < 20) return null;
  const w = el.clientWidth;
  return new uPlot(
    {
      width: w, height: 54,
      cursor: { show: false },
      legend: { show: false },
      scales: { x: { time: false } },
      series: [
        {},
        { stroke: "#e8a33d", width: 1.5, fill: "rgba(232,163,61,.15)" },
        { stroke: "#4cc38a", width: 1.5 },
      ],
      axes: [{ show: false }, { show: false }],
    },
    [[], []],
    el,
  );
}

function renderStatus(s) {
  $("#loopbar").innerHTML =
    `loop <b>${s.loop_kind}</b> · mode <b>${s.mode}</b> · target <b>${fmt(s.target_c)}</b>` +
    ` · tick <b>${s.interval_ms} ms</b> · GPUs <b>${s.gpus.length}</b>`;

  for (const g of s.gpus) {
    const el = gpuCard(g);
    el.querySelector(".name").textContent = g.name;
    const badge = el.querySelector(".badge");
    badge.textContent = g.failsafe;
    badge.className = `badge ${g.failsafe} mono`;
    el.querySelector(".gmode").textContent = g.mode;

    // fetch the newest history sample for the big numbers (cheap: 1s window)
    fetch(`/api/history?gpu=${g.index}&seconds=2`)
      .then((r) => r.json())
      .then((hist) => {
        const last = hist[hist.length - 1] ?? {};
        const temp = last.temp_c ?? g.temp_c;
        const tempEl = el.querySelector('[data-k="temp"]');
        tempEl.textContent = fmt(temp);
        tempEl.className = `v ${temp != null && s.target_c != null && temp > s.target_c + 2 ? "t-hot" : temp != null && s.target_c != null && temp > s.target_c - 3 ? "t-warm" : ""}`;
        el.querySelector('[data-k="pwr"]').textContent = fmt(last.power_w);
        el.querySelector('[data-k="clk"]').textContent = fmt(last.core_clock_mhz, 0);
        el.querySelector('[data-k="fan"]').textContent = fmt(last.fan_pct, 0);
        if (pageNow() === "fan" && g.index === 0) {
          $("#fan-live").textContent = fmt(last.fan_pct, 0);
          $("#fan-temp").textContent = fmt(last.temp_c);
        }
        if (el._spark) {
          el._spark.setData([
            hist.map((h) => h.t),
            hist.map((h) => h.temp_c),
            hist.map((h) => h.fan_pct),
          ]);
        }
      })
      .catch(() => {});
  }
}

/* ---------- big time-series charts ---------- */
async function loadHistory() {
  if (!chartsReady) return;
  const gpus = $$("#cards .gpu");
  if (!gpus.length) return;
  const first = gpus[0]?.id?.replace("gpu-", "");
  if (first == null) return;
  try {
    const hist = await (await fetch(`/api/history?gpu=${first}&seconds=${rangeSecs}`)).json();
    // uPlot time scales are in SECONDS.
    const ts = hist.map((h) => h.t);
    const data = (key) => [ts, ...hist.map((h) => h[key] ?? null)];
    if (charts.temp) charts.temp.setData([ts, hist.map((h) => h.temp_c)]);
    if (charts.power) charts.power.setData([ts, hist.map((h) => h.power_w)]);
    if (charts.clock) charts.clock.setData([ts, hist.map((h) => h.core_clock_mhz)]);
    if (charts.fan) charts.fan.setData([ts, hist.map((h) => h.fan_pct)]);
  } catch (e) { /* transient */ }
}

function initCharts() {
  if (!window.uPlot) return;
  const mk = (sel, title, unit, stroke) => {
    const el = $(sel);
    return new uPlot(
      {
        title, width: el.clientWidth || 800, height: 130,
        scales: { x: { time: true } },
        series: [{}, { stroke, width: 1.5, fill: `${stroke}22`, label: title }],
        axes: [
          { stroke: "#7f8ca3", grid: { stroke: "#1c2430" } },
          { stroke: "#7f8ca3", grid: { stroke: "#1c2430" }, label: unit },
        ],
      },
      [[], []],
      el,
    );
  };
  charts.temp = mk("#chart-temp", "Temperature", "°C", "#e5534b");
  charts.power = mk("#chart-power", "Power", "W", "#e8a33d");
  charts.clock = mk("#chart-clock", "Core clock", "MHz", "#539bf5");
  charts.fan = mk("#chart-fan", "Fan", "%", "#4cc38a");
  chartsReady = true;
  $("#range").addEventListener("click", (ev) => {
    const s = ev.target.dataset.s;
    if (!s) return;
    rangeSecs = +s;
    $$("#range button").forEach((b) => b.classList.toggle("on", b === ev.target));
    loadHistory();
  });
}

/* ---------- overclock page ---------- */
async function refreshOc() {
  const gpu = $("#oc-gpu").value ?? "0";
  try {
    const oc = await (await fetch(`/api/oc?gpu=${gpu}`)).json();
    $("#oc-core-cur").textContent = oc.core_offset_mhz ?? "—";
    $("#oc-mem-cur").textContent = oc.mem_offset_mhz ?? "—";
    if (oc.power) {
      $("#oc-power-hint").textContent = `${oc.power.min_w} … ${oc.power.max_w} W`;
      $("#oc-power").min = oc.power.min_w; $("#oc-power").max = oc.power.max_w;
      $("#oc-power").value = oc.power.current_w;
    }
    if (oc.temp) {
      $("#oc-temp-hint").textContent = `${oc.temp.min_c} … ${oc.temp.max_c} °C (slowdown)`;
      $("#oc-temp").min = oc.temp.min_c; $("#oc-temp").max = oc.temp.max_c;
      $("#oc-temp").value = oc.temp.current_c;
    }
  } catch (e) { toast("OC readback failed", true); }
}

function fillGpuPicks(status) {
  for (const sel of ["#oc-gpu", "#vf-gpu", "#about-gpu"]) {
    const el = $(sel);
    if (!el || el.options.length) continue;
    for (const g of status.gpus) {
      const opt = document.createElement("option");
      opt.value = g.index;
      opt.textContent = `${g.index}: ${g.name}`;
      el.appendChild(opt);
    }
  }
}

$("#oc-apply").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  const core = $("#oc-core").value, mem = $("#oc-mem").value;
  if (core === "" && mem === "") { toast("nothing to apply", true); return; }
  if (!(await confirmBox("Apply clock offsets",
    `core: ${core || "unchanged"} MHz · mem: ${mem || "unchanged"} MHz`))) return;
  if (core !== "") await post(`/api/oc/offset?gpu=${gpu}&domain=core&value=${core}`);
  if (mem !== "") await post(`/api/oc/offset?gpu=${gpu}&domain=mem&value=${mem}`);
  refreshOc();
});
$("#oc-reset").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  if (!(await confirmBox("Reset offsets", "Reset core and memory offsets to 0?"))) return;
  await post(`/api/reset?gpu=${gpu}&kind=offset_core`);
  await post(`/api/reset?gpu=${gpu}&kind=offset_mem`);
  refreshOc();
});
$("#power-apply").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  const w = $("#oc-power").value;
  if (w === "") { toast("enter a watt value", true); return; }
  if (await confirmBox("Apply power limit", `Set the power limit to ${w} W?`)) {
    await post(`/api/oc/power?gpu=${gpu}&watt=${w}`);
    refreshOc();
  }
});
$("#power-reset").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  if (await confirmBox("Reset power limit", "Restore the default power limit?")) {
    await post(`/api/reset?gpu=${gpu}&kind=power`);
    refreshOc();
  }
});
$("#temp-apply").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  const c = $("#oc-temp").value;
  if (c === "") { toast("enter a temperature", true); return; }
  if (await confirmBox("Apply temperature limit", `Set the temperature limit to ${c} °C?`)) {
    await post(`/api/oc/temp?gpu=${gpu}&c=${c}`);
    refreshOc();
  }
});
$("#temp-reset").addEventListener("click", async () => {
  const gpu = $("#oc-gpu").value ?? "0";
  if (await confirmBox("Reset temperature limit", "Restore the slowdown threshold?")) {
    await post(`/api/reset?gpu=${gpu}&kind=temp`);
    refreshOc();
  }
});

/* ---------- fan page ---------- */
$("#fan-pct").addEventListener("input", (ev) => {
  $("#fan-pct-v").textContent = ev.target.value;
});
$("#fan-pin").addEventListener("click", () =>
  post(`/fan?percent=${$("#fan-pct").value}`));
$("#fan-auto").addEventListener("click", () => post("/restore"));

/* ---------- v/f curve (read-only) ---------- */
async function refreshVf() {
  const gpu = $("#vf-gpu").value ?? "0";
  let pts;
  try {
    const j = await (await fetch(`/api/vfcurve?gpu=${gpu}`)).json();
    pts = j.points;
  } catch (e) {
    $("#vfchart").textContent = "V/F table unavailable";
    return;
  }
  if (!pts?.length) { $("#vfchart").textContent = "V/F table unavailable"; return; }
  const mv = pts.map((p) => p.mv);
  const mhz = pts.map((p) => p.mhz);
  if (!window.uPlot) { $("#vfchart").textContent = `${mv.length} points (chart unavailable)`; return; }
  if (vfChart) {
    vfChart.setData([mv, mhz]);
  } else {
    vfChart = new uPlot(
      {
        title: "core V/F (current plane, offset-inclusive)",
        width: $("#vfchart").clientWidth || 800,
        height: 380,
        scales: { x: { time: false } },
        series: [
          { label: "mV" },
          { label: "MHz", stroke: "#38b6ca", width: 2, points: { show: true, size: 4 } },
        ],
        axes: [
          { label: "voltage (mV)", stroke: "#7f8ca3", grid: { stroke: "#1c2430" } },
          { label: "frequency (MHz)", stroke: "#7f8ca3", grid: { stroke: "#1c2430" } },
        ],
      },
      [mv, mhz],
      $("#vfchart"),
    );
  }
}
$("#vf-refresh").addEventListener("click", refreshVf);

/* ---------- about ---------- */
async function refreshAbout() {
  const gpu = $("#about-gpu").value ?? "0";
  let info;
  try {
    info = await (await fetch(`/api/info?gpu=${gpu}`)).json();
  } catch (e) {
    $("#about-table").textContent = "info unavailable";
    return;
  }
  const flat = [];
  const walk = (obj, prefix = "") => {
    for (const [k, v] of Object.entries(obj)) {
      if (v == null) continue;
      if (typeof v === "object") walk(v, `${prefix}${k}.`);
      else flat.push([`${prefix}${k}`, String(v)]);
    }
  };
  walk(info);
  $("#about-table").innerHTML =
    `<table class="kv mono">${
      flat.map(([k, v]) => `<tr><td>${k}</td><td>${v}</td></tr>`).join("")
    }</table>`;
}

/* ---------- boot ---------- */
function setConn2(cls, text) { setConn(cls, text); }

(async function boot() {
  route();
  await loadConfig();
  await pollStatus();
  fillGpuPicks(await (await fetch("/api/status")).json());
  // uPlot needs a visible container to measure itself — init on first
  // Dashboard view (route() toggles display before this runs) and adapt
  // to resizes.
  if (location.hash === "#/dashboard" || !location.hash) initCharts();
  await loadHistory();
  setInterval(pollStatus, 1000);
  setInterval(() => {
    if (activePage() === "dashboard" && chartsReady) loadHistory();
  }, 5000);
  setInterval(loadConfig, 10000);
  window.addEventListener("resize", () => {
    if (!chartsReady) return;
    for (const chart of Object.values(charts)) {
      chart.setSize({ width: chart.root.parentElement.clientWidth });
    }
    for (const el of $$("#cards [data-spark]")) {
      if (el._spark) el._spark.setSize({ width: el.clientWidth });
    }
  });
})();

function activePage() {
  return (location.hash || "#/dashboard").replace("#/", "") || "dashboard";
}
const pageNow = activePage;
