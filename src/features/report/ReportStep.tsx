import { useCallback, useEffect, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { useWizard } from "../../store/wizard";
import { useRun } from "../../store/run";
import type { CrossTab } from "../../types/gen/CrossTab";
import type { ExportFormat } from "../../types/gen/ExportFormat";
import type { QuestionReport } from "../../types/gen/QuestionReport";
import type { Report } from "../../types/gen/Report";
import { AppShell } from "../../components/AppShell";
import { pillButton } from "../../components/fields";
import { Bars, Diverging, Histogram, Pie, Themes } from "./charts";

const smallButton =
  "inline-flex h-9 items-center justify-center rounded-full border border-line px-3.5 text-sm hover:bg-[#f1f1ee] disabled:cursor-not-allowed disabled:opacity-40";

function Chart({ q }: { q: QuestionReport }) {
  switch (q.chart) {
    case "pie":
      return <Pie rows={q.rows} />;
    case "bar":
      return <Bars rows={q.rows} />;
    case "multi_bar":
      return <Bars rows={q.rows} note="% of respondents choosing each option; people could pick more than one, so these can total more than 100%." />;
    case "diverging":
      return <Diverging rows={q.rows} mean={q.mean} />;
    case "histogram":
      return <Histogram rows={q.rows} median={q.median} q1={q.q1} q3={q.q3} unit={q.unit} />;
    case "themes":
      return <Themes themes={q.themes} samples={q.sampleAnswers} n={q.n} />;
  }
}

function CrossTabTable({ t }: { t: CrossTab }) {
  const pctCell = (v: number) => `${v % 1 === 0 ? v.toFixed(0) : v.toFixed(1)}%`;
  const hasMean = t.groups.some((g) => g.mean != null);
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-sm">
        <thead>
          <tr className="text-left text-xs text-muted">
            <th className="py-1.5 pr-3 font-medium">{t.dimension.label}</th>
            <th className="px-2 py-1.5 text-right font-medium">n</th>
            {t.columns.map((c) => <th key={c.key} className="max-w-[140px] truncate px-2 py-1.5 text-right font-medium" title={c.label}>{c.label}</th>)}
            {hasMean && <th className="px-2 py-1.5 text-right font-medium">Mean</th>}
          </tr>
        </thead>
        <tbody>
          {t.groups.map((g) => (
            <tr key={g.label} className={`border-t border-[#efefea] ${g.lowBase ? "text-muted" : ""}`}>
              <td className="py-1.5 pr-3">
                {g.label}
                {g.lowBase && <span className="ml-2 rounded-full bg-[#f6ecd9] px-2 py-0.5 text-[11px] text-[#7a5a1c]" title="Fewer than 30 respondents: read with care">low base</span>}
              </td>
              <td className="px-2 py-1.5 text-right font-mono text-xs">{g.n}</td>
              {g.cells.map((v, i) => <td key={i} className="px-2 py-1.5 text-right">{pctCell(v)}</td>)}
              {hasMean && <td className="px-2 py-1.5 text-right">{g.mean?.toFixed(2) ?? "—"}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function QuestionCard({ runId, q, dimensions }: { runId: number; q: QuestionReport; dimensions: Report["dimensions"] }) {
  const [dim, setDim] = useState("");
  const [tab, setTab] = useState<CrossTab | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    setTab(null);
    setError("");
    if (!dim) return;
    api.getCrosstab(runId, q.questionId, dim).then(setTab).catch((e) => setError(errorMessage(e)));
  }, [runId, q.questionId, dim]);
  const canBreak = q.chart !== "themes" || q.themes.length > 0;
  return (
    <article className="flex flex-col gap-4 rounded-[26px] border border-line bg-white p-6">
      <header className="flex items-start justify-between gap-4">
        <div>
          <span className="font-mono text-xs text-muted">{q.code}</span>
          <h3 className="m-0 mt-1 font-display text-lg font-medium">{q.text}</h3>
        </div>
        <span className="shrink-0 text-right text-xs text-muted">
          n = {q.n}
          {(q.invalid > 0 || q.refused > 0) && <><br />{q.invalid ? `${q.invalid} invalid` : ""}{q.invalid && q.refused ? " · " : ""}{q.refused ? `${q.refused} refused` : ""}</>}
        </span>
      </header>
      <Chart q={q} />
      {q.validity.flags.length > 0 && (
        <ul className="m-0 flex list-none flex-col gap-1 rounded-2xl bg-[#faf5e6] p-3 text-sm text-[#5c4a14]" aria-label="Response quality flags">
          {q.validity.flags.map((f) => <li key={f}>⚠ {f}</li>)}
        </ul>
      )}
      {(q.validity.entropy != null || q.validity.firstPositionRate != null) && (
        <p className="m-0 text-xs text-muted">
          {q.validity.entropy != null && `Spread (entropy) ${q.validity.entropy.toFixed(2)} of 1`}
          {q.validity.midpointRate != null && ` · midpoint ${q.validity.midpointRate}%`}
          {q.validity.firstPositionRate != null && ` · chose the option shown first ${q.validity.firstPositionRate}%, last ${q.validity.lastPositionRate}%`}
        </p>
      )}
      {canBreak && dimensions.length > 0 && (
        <div className="flex flex-col gap-2 border-t border-[#efefea] pt-3">
          <label className="flex items-center gap-2 text-sm text-muted">
            Break down by
            <select className="h-9 rounded-full border border-line bg-white px-3 text-sm text-ink" value={dim} onChange={(e) => setDim(e.target.value)}>
              <option value="">—</option>
              {dimensions.map((d) => <option key={d.key} value={d.key}>{d.label}</option>)}
            </select>
          </label>
          {error && <p className="m-0 text-sm text-[#8a3a26]">{error}</p>}
          {tab && <CrossTabTable t={tab} />}
        </div>
      )}
    </article>
  );
}

/** Step 5 (docs/DATA_FLOW.md §3, Step 5): results, AI synthesis and offline exports. */
export function ReportStep() {
  const { info, projectId, goTo } = useWizard();
  const { run } = useRun();
  const [runId, setRunId] = useState<number | null>(run?.id ?? null);
  const [report, setReport] = useState<Report | null>(null);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (runId != null || projectId == null) return;
    api.getLatestRun(projectId).then((r) => r && setRunId(r.id)).catch((e) => setError(errorMessage(e)));
  }, [runId, projectId]);

  const load = useCallback(async () => {
    if (runId == null) return;
    try {
      setReport(await api.getReport(runId));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [runId]);
  useEffect(() => {
    load();
  }, [load]);
  const writing = report?.synthesisStatus === "generating";
  useEffect(() => {
    if (!writing) return;
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, [writing, load]);

  async function regenerate() {
    if (runId == null) return;
    setError("");
    try {
      setReport(await api.regenerateSynthesis(runId));
    } catch (e) {
      setError(errorMessage(e));
    }
  }
  async function exportAs(format: ExportFormat) {
    if (runId == null) return;
    setError("");
    setSaved("");
    setBusy(true);
    try {
      const path = await api.exportRun(runId, format);
      if (path) setSaved(`Saved to ${path}`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const r = report?.run;
  const partial = r && r.status === "stopped";
  const s = report?.synthesis;
  const dimLabel = (key: string) => report?.dimensions.find((d) => d.key === key)?.label ?? key;

  return (
    <AppShell
      title={info.title || "Project"}
      heading="Report"
      subtitle="Results from the simulated respondents, an AI summary checked against the numbers, and offline exports."
      footer={<button type="button" className={pillButton} onClick={() => goTo(3)}>Back</button>}
    >
      <div role="note" className="rounded-2xl border border-[#d9cfae] bg-[#faf5e6] px-5 py-3 text-[15px] text-[#5c4a14]">
        <strong className="font-medium">Synthetic respondents — directional only; not calibrated against real survey data.</strong> Every answer was written by an AI
        model playing a persona; these are not real people.{report?.run && ` Model ${report.run.model}, prompt ${report.run.promptVersion}.`}
      </div>
      {error && <div role="alert" className="rounded-2xl border border-[#e7b4a8] bg-[#fbeee9] px-5 py-3 text-[15px] text-[#8a3a26]">{error}</div>}
      {r && (
        <p className="m-0 text-sm text-muted">
          {partial ? `Stopped early: based on ${report!.basedOnN} of ${r.respondents} respondents.` : `Based on ${report!.basedOnN} respondents.`}

        </p>
      )}
      {!report && !error && <p className="m-0 text-muted">{runId == null ? "Run a simulation first." : "Loading the report…"}</p>}

      {report && r && (
        <div className="grid grid-cols-[1fr_340px] items-start gap-6">
          <section aria-label="Results by question" className="flex flex-col gap-5">
            {report.questions.map((q) => <QuestionCard key={q.questionId} runId={r.id} q={q} dimensions={report.dimensions} />)}
          </section>

          <aside className="sticky top-6 flex flex-col gap-5">
            <section aria-label="AI synthesis" className="flex flex-col gap-3 rounded-[26px] border border-line bg-white p-6">
              <div className="flex items-center justify-between">
                <h2 className="m-0 font-display text-lg font-semibold">AI synthesis</h2>
                <button type="button" className={smallButton} onClick={regenerate} disabled={writing || r.respondentsDone === 0}>Regenerate</button>
              </div>
              {writing && <p className="m-0 text-sm text-muted" role="status">Coding open answers and writing the summary…</p>}
              {report.synthesisStatus === "failed" && <p className="m-0 text-sm text-[#8a3a26]">Couldn't generate: {report.synthesisError}. The results on the left don't depend on it; try Regenerate.</p>}
              {s && !writing && (
                <>
                  <p className="m-0 text-[15px] leading-relaxed">{s.summary || "No summary sentence passed the checks."}</p>
                  {s.frictionPoints.length > 0 && (
                    <div>
                      <h3 className="m-0 mb-1.5 text-sm font-medium">Friction points</h3>
                      <ul className="m-0 flex list-none flex-col gap-1 p-0 text-sm">
                        {s.frictionPoints.map((f) => (
                          <li key={f.label} className="flex justify-between gap-3"><span>{f.label}</span><span className="font-mono text-xs text-muted">{f.mentions} mentions</span></li>
                        ))}
                      </ul>
                    </div>
                  )}
                  {s.segments.length > 0 && (
                    <div>
                      <h3 className="m-0 mb-1.5 text-sm font-medium">By group</h3>
                      <ul className="m-0 flex list-none flex-col gap-2 p-0 text-sm">
                        {s.segments.map((g, i) => (
                          <li key={i}><span className="text-muted">{dimLabel(g.dimension)}: {g.group} — </span>{g.takeaway}</li>
                        ))}
                      </ul>
                    </div>
                  )}
                  <p className="m-0 text-xs text-muted">
                    Based on {s.basedOnN} respondents · {s.model}. Every count and group above was checked against the results
                    {s.dropped > 0 ? `; ${s.dropped} claim${s.dropped === 1 ? "" : "s"} that failed the check ${s.dropped === 1 ? "was" : "were"} removed.` : "."}
                  </p>
                </>
              )}
            </section>

            <section aria-label="Offline export" className="flex flex-col gap-3 rounded-[26px] border border-line bg-white p-6">
              <h2 className="m-0 font-display text-lg font-semibold">Offline export</h2>
              <p className="m-0 text-sm text-muted">Saved to this computer only. CSV opens in Excel (one row per respondent); JSON holds everything, including the review history and reasons.</p>
              <div className="flex gap-3">
                <button type="button" className={smallButton} onClick={() => exportAs("csv")} disabled={busy}>Export CSV</button>
                <button type="button" className={smallButton} onClick={() => exportAs("json")} disabled={busy}>Export JSON</button>
              </div>
              {saved && <p className="m-0 break-all text-sm text-[#3d5a2a]" role="status">{saved}</p>}
            </section>
          </aside>
        </div>
      )}
    </AppShell>
  );
}
