import { useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { useWizard } from "../../store/wizard";
import { useRun } from "../../store/run";
import type { Question } from "../../types/gen/Question";
import type { Survey } from "../../types/gen/Survey";
import { AppShell } from "../../components/AppShell";
import { Arrow, pillButton, primaryButton } from "../../components/fields";
import { usd } from "../../lib/cost";

const time = (ms: string) => {
  const n = Number(ms);
  return Number.isFinite(n) && n > 0 ? new Date(n).toLocaleTimeString() : "";
};

/** Bars for one question from the live answer counts. */
function Chart({ q, counts }: { q: Question; counts: Record<string, number> }) {
  const b = q.body;
  let rows: [string, number][];
  if (b.options.length) rows = b.options.map((o) => [o.label, counts[o.code] ?? 0]);
  else if (b.scale) {
    rows = [];
    for (let v = b.scale.min; v <= b.scale.max; v++) rows.push([String(v), counts[String(v)] ?? 0]);
  } else if (b.numeric) {
    // Five equal ranges.
    const { min, max } = b.numeric;
    const step = (max - min) / 5;
    rows = [0, 1, 2, 3, 4].map((i) => {
      const lo = min + i * step;
      const hi = i === 4 ? max : lo + step;
      const n = Object.entries(counts).reduce((a, [v, c]) => {
        const x = Number(v);
        return a + (x >= lo && (x < hi || (i === 4 && x <= hi)) ? c : 0);
      }, 0);
      return [`${Math.round(lo)}–${Math.round(hi)}`, n];
    });
  } else return null;
  const total = rows.reduce((a, [, n]) => a + n, 0);
  const top = Math.max(1, ...rows.map(([, n]) => n));
  return (
    <figure className="m-0 flex flex-col gap-2 rounded-2xl border border-line bg-white p-4">
      <figcaption className="flex justify-between gap-3 text-sm">
        <span className="font-medium">{q.code} · {b.text}</span>
        <span className="shrink-0 text-muted">n = {total}</span>
      </figcaption>
      {rows.map(([label, n]) => (
        <div key={label} className="grid grid-cols-[140px_1fr_44px] items-center gap-2 text-xs">
          <span className="truncate" title={label}>{label}</span>
          <span className="h-3 rounded-full bg-[#efefea]"><span className="block h-3 rounded-full bg-accent transition-all" style={{ width: `${(n / top) * 100}%` }} /></span>
          <span className="text-right font-mono text-muted">{total ? Math.round((n / total) * 100) : 0}%</span>
        </div>
      ))}
    </figure>
  );
}

/** Step 4 (docs/DATA_FLOW.md §3, Step 4): live progress of the simulation run. */
export function SimulationStep() {
  const { info, projectId, goTo, reach } = useWizard();
  const { run, status, stats, log, counts, setRun, apply } = useRun();
  const [survey, setSurvey] = useState<Survey | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const consoleRef = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);

  useEffect(() => {
    if (projectId == null) return;
    api.getSurvey(projectId).then(setSurvey).catch((e) => setError(errorMessage(e)));
    // Coming back to this step (or after a relaunch): show the stored run.
    if (!run) api.getLatestRun(projectId).then((r) => r && setRun(r)).catch(() => {});
  }, [projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  // While a run is paused or finished, refresh its stored counts (and any error it stopped with).
  useEffect(() => {
    if (projectId == null || status === "running") return;
    api.getLatestRun(projectId).then((r) => r && setRun(r)).catch(() => {});
  }, [status, projectId]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (follow && consoleRef.current) consoleRef.current.scrollTop = consoleRef.current.scrollHeight;
  }, [log, follow]);

  const running = status === "running";
  const answered = stats?.answered ?? run?.answered ?? 0;
  const totalAnswers = stats?.totalAnswers ?? (run ? run.respondents * run.questions : 0);
  const done = stats?.respondentsDone ?? run?.respondentsDone ?? 0;
  const pct = totalAnswers ? Math.min(100, Math.round((answered / totalAnswers) * 100)) : 0;

  async function act(f: () => Promise<void>) {
    setError("");
    setBusy(true);
    try {
      await f();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }
  const pause = () => act(async () => { if (run) await api.pauseRun(run.id); });
  const resume = () => act(async () => { if (run) setRun(await api.resumeRun(run.id, apply)); });
  const stop = () => act(async () => { if (run) setRun(await api.stopRun(run.id)); });

  const cards: [string, string, string][] = [
    ["Answers collected", `${answered.toLocaleString()} / ${totalAnswers.toLocaleString()}`, `${pct}%`],
    ["Respondents complete", `${done} / ${run?.respondents ?? "—"}`, run ? `${run.questions} questions each` : ""],
    ["API cost so far", usd(stats?.costUsd ?? run?.costUsd), (stats?.costUsd ?? run?.costUsd) == null ? `No price for ${run?.model ?? "this model"}` : run?.estCostUsd != null ? `of ≈ ${usd(run.estCostUsd)} estimated` : "from token counts"],
    ["Latency", stats ? `${(stats.avgLatencyMs / 1000).toFixed(1)} s` : "—", stats ? `p95 ${(stats.p95LatencyMs / 1000).toFixed(1)} s` : "average per call"],
    ["Throughput", stats ? `${stats.answersPerMin}/min` : "—", stats ? `${stats.concurrency} calls at once` : "answers in the last minute"],
  ];
  const statusText: Record<string, string> = {
    running: "Running",
    paused: "Paused",
    stopped: "Stopped and saved",
    completed: "Complete",
    failed: "Failed",
    queued: "Starting",
    cancelled: "Cancelled",
  };

  return (
    <AppShell
      title={info.title || "Project"}
      heading="Live Simulation"
      subtitle="Gemini answers as each persona. Answers are saved as they arrive; pausing or closing the app loses nothing."
      footer={
        <>
          <button type="button" className={pillButton} onClick={() => goTo(2)} disabled={running}>Back</button>
          <div className="flex gap-3">
            {running && <button type="button" className={pillButton} onClick={pause} disabled={busy}>Pause Simulation</button>}
            {status === "paused" && <button type="button" className={pillButton} onClick={resume} disabled={busy}>Resume</button>}
            {(running || status === "paused") && <button type="button" className={pillButton} onClick={stop} disabled={busy}>Stop &amp; Save Progress</button>}
            <button type="button" className={primaryButton} disabled={status !== "completed" && status !== "stopped"} onClick={() => reach(4)}>
              View Report <Arrow />
            </button>
          </div>
        </>
      }
    >
      <div className="flex flex-col gap-2" role="status" aria-live="polite">
        <div className="flex justify-between text-sm text-muted">
          <span>{status ? statusText[status] : "No run yet"}{run ? ` · ${run.model}` : ""}</span>
          <span>{pct}%</span>
        </div>
        <div className="h-2.5 rounded-full bg-[#efefea]"><div className="h-2.5 rounded-full bg-accent transition-all" style={{ width: `${pct}%` }} /></div>
      </div>
      {(error || (status === "paused" && run?.error)) && (
        <div role="alert" className="rounded-2xl border border-[#e7b4a8] bg-[#fbeee9] px-5 py-3 text-[15px] text-[#8a3a26]">{error || run?.error}</div>
      )}

      <div className="grid grid-cols-5 gap-4">
        {cards.map(([label, value, sub]) => (
          <div key={label} className="flex flex-col gap-1 rounded-[22px] border border-line bg-white px-5 py-4">
            <span className="text-sm text-muted">{label}</span>
            <span className="font-display text-2xl font-semibold">{value}</span>
            <span className="text-xs text-muted">{sub}</span>
          </div>
        ))}
      </div>

      <div className="grid min-h-[420px] grid-cols-[1.1fr_1fr] gap-6">
        <section aria-label="Live console" className="flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <h2 className="m-0 font-display text-lg font-semibold">Live console</h2>
            <label className="flex items-center gap-2 text-sm text-muted">
              <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Follow
            </label>
          </div>
          <div ref={consoleRef} className="h-[420px] overflow-y-auto rounded-2xl bg-[#1f1f1c] p-4 font-mono text-xs leading-relaxed text-[#e8e8e2]">
            {log.length === 0 && <p className="m-0 text-[#9a9a92]">{running ? "Waiting for the first answers…" : "Answers appear here while the run is going."}</p>}
            {log.map((l) =>
              l.kind === "answer" ? (
                <p key={l.key} className="m-0">
                  <span className="text-[#9a9a92]">{time(l.line.at)} </span>
                  [Respondent #{l.line.respondent}] answered {l.line.question}: <span className="text-[#c9e2b3]">{l.line.answer}</span>
                  {l.line.reason && <span className="text-[#b5b5ad]"> because {l.line.reason}</span>}
                </p>
              ) : (
                <p key={l.key} className={`m-0 ${l.level === "error" ? "text-[#f0a591]" : l.level === "warn" ? "text-[#f0d091]" : "text-[#9ac2e6]"}`}>
                  <span className="text-[#9a9a92]">{time(l.at)} </span>{l.text}
                </p>
              ),
            )}
          </div>
        </section>
        <section aria-label="Live charts" className="flex flex-col gap-2">
          <h2 className="m-0 font-display text-lg font-semibold">Live charts</h2>
          <div className="flex h-[420px] flex-col gap-3 overflow-y-auto pr-1">
            {survey?.questions.map((q) => <Chart key={q.id} q={q} counts={counts[q.id] ?? {}} />)}
            {Object.keys(counts).length === 0 && <p className="m-0 text-sm text-muted">Charts fill in from this session's answers. The report shows the full results.</p>}
          </div>
        </section>
      </div>
    </AppShell>
  );
}
