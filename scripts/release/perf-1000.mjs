// TEST_PLAN S10 `stream_fps_1000` and the UI half of S7 `memory_1000` (BACKLOG B14).
// Drives a performance-harness build of the app over the WebView2 DevTools protocol:
// opens the seeded project on Step 3, clicks Run Survey Simulation, records every frame
// while 1,000 respondents stream in, then browses the report (every cross-tab) and the
// other steps. perf-1000.ps1 starts the app and samples memory meanwhile.
//
// Usage: node perf-1000.mjs --project <id> --out <ui.json> [--port 9222]
// Needs Node 22+ (built-in fetch and WebSocket); no packages.

import { writeFileSync } from "node:fs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const port = Number(arg("--port", "9222"));
const project = Number(arg("--project"));
const out = arg("--out", "ui.json");
if (!project) throw new Error("--project <id> is required");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (m) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${m}`);

async function pageTarget() {
  const until = Date.now() + 120_000;
  let last = "nothing listening";
  while (Date.now() < until) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page" && t.webSocketDebuggerUrl);
      if (page) return page;
      last = `targets: ${JSON.stringify(list.map((t) => ({ type: t.type, url: t.url })))}`;
    } catch (e) {
      last = `${e.message}${e.cause ? ` (${e.cause.code ?? e.cause.message})` : ""}`;
    }
    await sleep(500);
  }
  throw new Error(`no WebView2 page on port ${port} after 120 s; last: ${last}`);
}

const target = await pageTarget();
log(`connected to ${target.url}`);
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = () => reject(new Error("DevTools WebSocket failed"));
});
let nextId = 1;
const pending = new Map();
ws.onmessage = (e) => {
  const msg = JSON.parse(e.data);
  const p = pending.get(msg.id);
  if (!p) return;
  pending.delete(msg.id);
  if (msg.error) p.reject(new Error(msg.error.message));
  else p.resolve(msg.result);
};
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });

async function evaluate(expression) {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}\n  in: ${expression}`);
  return r.result.value;
}

async function waitFor(expression, label, timeoutMs = 60_000) {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    const v = await evaluate(expression);
    if (v) return v;
    await sleep(250);
  }
  throw new Error(`timed out after ${timeoutMs / 1000} s waiting for ${label}`);
}

const button = (text) => `[...document.querySelectorAll("button")].find((b) => b.textContent.includes(${JSON.stringify(text)}))`;

await send("Page.bringToFront").catch(() => {});
await waitFor(`typeof window.__perf === "object"`, "the harness hooks (build with VITE_PERF_HARNESS=1)");
await evaluate(`window.__perf.open(${project}, "Performance run", 2)`);
await waitFor(`(() => { const b = ${button("Run Survey Simulation")}; return b && !b.disabled; })()`, "Run Survey Simulation to enable");

// Frame recorder: the time between animation frames, and long tasks. A frame counts as on
// time when it arrives within 1.5 vsync intervals of the last one (no vsync missed).
await evaluate(`(() => {
  const rec = { frames: [], longTasks: [], on: true };
  window.__rec = rec;
  let last = 0;
  const tick = (t) => {
    if (!rec.on) return;
    if (last) rec.frames.push(t - last);
    last = t;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
  new PerformanceObserver((l) => { for (const e of l.getEntries()) rec.longTasks.push(Math.round(e.duration)); })
    .observe({ type: "longtask" });
})()`);

log("starting the run");
const started = Date.now();
await evaluate(`${button("Run Survey Simulation")}.click()`);
const statusText = `(document.querySelector('[role="status"] span')?.textContent ?? "")`;
const final = await waitFor(
  `(() => {
    const s = ${statusText};
    if (s.startsWith("Complete")) return "completed";
    if (s.startsWith("Failed") || s.startsWith("Paused")) return "stopped: " + s + " " + (document.querySelector('[role="alert"]')?.textContent ?? "");
    return "";
  })()`,
  "the run to complete",
  15 * 60_000,
);
const runSeconds = (Date.now() - started) / 1000;
const rec = await evaluate(`(() => { window.__rec.on = false; return { frames: window.__rec.frames, longTasks: window.__rec.longTasks }; })()`);
if (final !== "completed") throw new Error(`the run did not complete: ${final}`);
log(`run completed in ${runSeconds.toFixed(1)} s`);

const vsync = 1000 / 60;
const frames = rec.frames;
const sorted = [...frames].sort((a, b) => a - b);
const pct = (p) => (sorted.length ? Math.round(sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))] * 10) / 10 : null);
const fps = {
  frames: frames.length,
  runSeconds: Math.round(runSeconds * 10) / 10,
  averageFps: runSeconds ? Math.round((frames.length / runSeconds) * 10) / 10 : 0,
  onTimePct: frames.length ? Math.round((frames.filter((d) => d <= vsync * 1.5).length / frames.length) * 1000) / 10 : 0,
  p50FrameMs: pct(50),
  p95FrameMs: pct(95),
  maxFrameMs: pct(100),
  longTasks: rec.longTasks.length,
  longTasksOver100ms: rec.longTasks.filter((d) => d > 100).length,
  maxLongTaskMs: rec.longTasks.length ? Math.max(...rec.longTasks) : 0,
};
log(`frames ${fps.frames}, on time ${fps.onTimePct}%, p95 ${fps.p95FrameMs} ms, long tasks > 100 ms: ${fps.longTasksOver100ms}`);

// Browse every screen: the report with every cross-tab open, then each step.
await waitFor(`(() => { const b = ${button("View Report")}; return b && !b.disabled; })()`, "View Report to enable");
await evaluate(`${button("View Report")}.click()`);
const cards = await waitFor(
  `document.querySelectorAll('section[aria-label="Results by question"] article').length`,
  "the report",
);
const selects = await evaluate(`(() => {
  const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set;
  const all = [...document.querySelectorAll('section[aria-label="Results by question"] select')];
  for (const s of all) {
    if (s.options.length < 2) continue;
    set.call(s, s.options[1].value);
    s.dispatchEvent(new Event("change", { bubbles: true }));
  }
  return all.length;
})()`);
const tables = await waitFor(
  `(() => { const n = document.querySelectorAll('section[aria-label="Results by question"] table').length; return n >= ${selects} ? n : 0; })()`,
  `${selects} cross-tabs`,
).catch(() => evaluate(`document.querySelectorAll('section[aria-label="Results by question"] table').length`));
log(`report: ${cards} questions, ${tables} of ${selects} cross-tabs shown`);
await sleep(3000);
for (const step of [1, 2, 3, 0, 4]) {
  await evaluate(`window.__perf.goTo(${step})`);
  await sleep(3000);
}
log("browsed every step");

writeFileSync(out, JSON.stringify({ fps, report: { questions: cards, crossTabs: tables, selects } }, null, 2));
ws.close();
process.exit(0);
