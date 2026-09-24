import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { cohortIsOutOfDate } from "../../lib/cohortDrift";
import { useWizard } from "../../store/wizard";
import type { CohortSummary } from "../../types/gen/CohortSummary";
import type { RespondentCard } from "../../types/gen/RespondentCard";
import type { RespondentDetail } from "../../types/gen/RespondentDetail";
import { AppShell } from "../../components/AppShell";
import { Arrow, pillButton, primaryButton } from "../../components/fields";

const PAGE = 24;

/** Step 2 (docs/DATA_FLOW.md §3, Step 2). */
export function PersonasStep() {
  const { info, cohort, currentCohort, progress, setCurrentCohort, setProgress, reach, goTo } = useWizard();
  const [summary, setSummary] = useState<CohortSummary | null>(null);
  const [cards, setCards] = useState<RespondentCard[]>([]);
  const [total, setTotal] = useState(0);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<RespondentDetail | null>(null);
  const [error, setError] = useState("");
  const cohortId = currentCohort?.id ?? null;
  const status = summary?.status ?? currentCohort?.status ?? "generating";
  const generating = status === "generating";
  const outOfDate =
    (status === "ready" || status === "locked") &&
    currentCohort != null &&
    cohortIsOutOfDate(info.countries, cohort, currentCohort);

  const refresh = useCallback(async () => {
    if (cohortId == null) return;
    try {
      const [s, page] = await Promise.all([api.getCohortSummary(cohortId), api.listRespondents(cohortId, query, 0, Math.max(PAGE, cards.length))]);
      setSummary(s);
      setCards(page.items);
      setTotal(page.total);
      if (s.status === "failed") {
        const latest = await api.getLatestCohort(currentCohort!.projectId);
        setError(latest?.error ?? "Persona generation failed.");
      }
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [cohortId, query, cards.length]);

  // Poll while generating (the progress channel drives the bar; polling fills the grid and cards).
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    refreshRef.current();
    if (!generating) return;
    const t = setInterval(() => refreshRef.current(), 1500);
    return () => clearInterval(t);
  }, [cohortId, generating]);
  useEffect(() => {
    const t = setTimeout(() => refreshRef.current(), 250);
    return () => clearTimeout(t);
  }, [query]);

  async function loadMore() {
    if (cohortId == null) return;
    const page = await api.listRespondents(cohortId, query, cards.length, PAGE);
    setCards([...cards, ...page.items]);
  }

  async function regenerate() {
    if (cohortId == null) return;
    setError("");
    setSummary(null);
    setCards([]);
    try {
      setProgress({ done: 0, total: currentCohort!.config.size, replaced: 0 });
      setCurrentCohort(await api.regenerateCohort(cohortId, setProgress));
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function proceed() {
    if (cohortId == null) return;
    try {
      setCurrentCohort(await api.lockCohort(cohortId));
      reach(2);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const pct = progress && progress.total ? Math.round((progress.done / progress.total) * 100) : 0;
  const gender = summary?.gender.map((g) => `${Math.round(g.percent)}% ${g.label.toLowerCase()}`).join(" · ") || "—";
  const metrics: [string, string, string][] = [
    ["Respondents", String(summary?.respondents ?? 0), generating ? `of ${currentCohort?.config.size ?? "—"} so far` : "All quotas matched"],
    ["Average age", summary?.averageAge != null ? summary.averageAge.toFixed(1) : "—", "Years"],
    ["Gender ratio", gender, "From census data"],
    [`Top ${summary?.topTriggerLabel.toLowerCase() ?? "trigger"}`, summary?.topTrigger?.label ?? "—", summary?.topTrigger ? `${Math.round(summary.topTrigger.percent)}% of respondents` : ""],
    ["Replaced at screening", String(summary?.replacedAtScreening ?? 0), "Failed the screening criteria"],
  ];

  return (
    <AppShell
      title={info.title || "Project"}
      heading="Persona Review"
      subtitle="Check who is in the cohort before writing questions. Select anyone to see their full profile."
      footer={
        <>
          <button type="button" className={pillButton} onClick={() => goTo(0)}>Back</button>
          <div className="flex gap-3">
            <button type="button" className={pillButton} disabled={generating} onClick={regenerate}>Regenerate Cohort</button>
            <button type="button" className={primaryButton} disabled={status !== "ready" && status !== "locked"} onClick={proceed}>
              Proceed to Questionnaire <Arrow />
            </button>
          </div>
        </>
      }
    >
      {generating && (
        <div className="flex flex-col gap-2" role="status" aria-live="polite">
          <div className="flex justify-between text-sm text-muted">
            <span>Writing personas with Gemini… {progress ? `${progress.done} of ${progress.total}` : ""}</span>
            <span>{progress?.replaced ? `${progress.replaced} replaced at screening` : ""}</span>
          </div>
          <div className="h-2.5 rounded-full bg-[#efefea]"><div className="h-2.5 rounded-full bg-accent transition-all" style={{ width: `${pct}%` }} /></div>
        </div>
      )}
      {error && (
        <div role="alert" className="flex items-center justify-between gap-4 rounded-full border border-[#f0c6be] bg-[#fbe9e6] px-6 py-3 text-bad">
          <span>{error}</span>
          <button type="button" className={pillButton} onClick={regenerate}>Try again</button>
        </div>
      )}
      {!error && outOfDate && (
        <div role="alert" className="flex items-center justify-between gap-4 rounded-full border border-[#e9d7ab] bg-[#fbf0dc] px-6 py-3 text-[#7a4a06]">
          <span>Cohort out of date — Step 1&rsquo;s audience has changed since this cohort was generated.</span>
          <button type="button" className={pillButton} onClick={regenerate}>Regenerate Cohort</button>
        </div>
      )}
      <div className="grid grid-cols-5 gap-3.5">
        {metrics.map(([label, value, note]) => (
          <div key={label} className="flex flex-col gap-1.5 rounded-[22px] border border-[#e2e2dc] bg-white px-5 py-4">
            <span className="font-display text-[13px] font-medium text-muted">{label}</span>
            <span className="font-display text-2xl font-semibold">{value}</span>
            <span className="text-[13px] text-muted">{note}</span>
          </div>
        ))}
      </div>
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <label htmlFor="persona-search" className="sr-only">Search respondents</label>
          <input id="persona-search" type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search by name, occupation or bias" className="h-12 w-96 rounded-full border border-line bg-white px-6" />
          <span className="text-sm text-muted">Showing {cards.length} of {total}</span>
        </div>
      </div>
      <div className="grid grid-cols-4 gap-3.5">
        {cards.map((c) => (
          <button
            key={c.id}
            type="button"
            onClick={async () => setOpen(await api.getRespondent(c.id))}
            className={`flex flex-col gap-2 rounded-[20px] border bg-white p-4 text-left hover:border-line ${open?.id === c.id ? "border-accent ring-1 ring-accent" : "border-[#e2e2dc]"}`}
          >
            <span className="flex items-baseline justify-between"><span className="font-display font-semibold">{c.name}</span><span className="font-mono text-xs text-muted">R-{String(c.ordinal).padStart(3, "0")}</span></span>
            <span className="text-sm">{c.occupation} · {c.age}</span>
            <span className="text-[13px] text-muted">{c.biases.join(" · ")}</span>
          </button>
        ))}
      </div>
      {cards.length < total && <button type="button" className={`${pillButton} self-center`} onClick={loadMore}>Show more</button>}
      {open && <Drawer person={open} onClose={() => setOpen(null)} />}
    </AppShell>
  );
}

function Drawer({ person: p, onClose }: { person: RespondentDetail; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  const section = (title: string, body: string) => (
    <div className="flex flex-col gap-1.5">
      <span className="font-display text-sm font-medium">{title}</span>
      <p className="m-0 text-[15px] leading-relaxed">{body}</p>
    </div>
  );
  return (
    <div className="fixed inset-0 z-40" onClick={onClose}>
      <div className="absolute inset-0 bg-ink/25" />
      <aside role="dialog" aria-label={`Profile of ${p.name}`} onClick={(e) => e.stopPropagation()} className="absolute inset-y-0 right-0 flex w-[480px] flex-col gap-5 overflow-y-auto bg-white p-9 shadow-2xl">
        <div className="flex items-start justify-between">
          <div className="flex flex-col gap-1">
            <span className="font-mono text-xs text-muted">R-{String(p.ordinal).padStart(3, "0")}</span>
            <h2 className="m-0 font-display text-[26px] font-semibold">{p.name}</h2>
            <span>{p.gender}, {p.age} · {p.occupation}</span>
            <span className="text-sm text-muted">{[p.region, p.country].filter(Boolean).join(", ")} · {p.income}</span>
          </div>
          <button type="button" aria-label="Close profile" onClick={onClose} className="h-10 w-10 rounded-full border border-[#b9b9b3] text-lg">×</button>
        </div>
        {section("Psychographic background", p.summary)}
        {section("Habits", `${p.habits} ${p.mediaHabits}`)}
        {section("Brand loyalties", p.brandLoyalties)}
        {section("Attitude to the category", p.categoryAttitudes)}
        <div className="flex flex-col gap-2">
          <span className="font-display text-sm font-medium">Key biases and values</span>
          <div className="flex flex-wrap gap-1.5">
            {[...p.biases, ...p.values].map((b) => <span key={b} className="rounded-full bg-[#ededE8] px-3 py-1 text-xs">{b}</span>)}
            <span className="rounded-full bg-[#eef1fd] px-3 py-1 text-xs text-accent-dark">Price sensitivity {p.priceSensitivity}/5</span>
          </div>
        </div>
        <dl className="m-0 grid grid-cols-2 gap-x-4 gap-y-1.5 text-sm">
          {p.categoryFacts.map(([k, v]) => (
            <div key={k} className="contents"><dt className="text-muted">{k}</dt><dd className="m-0">{v}</dd></div>
          ))}
        </dl>
        <div className={`rounded-[18px] px-4 py-3 text-sm ${p.screenStatus === "flagged" ? "bg-[#fbf0dc] text-[#7a4a06]" : "bg-ok-bg text-ok"}`}>
          <strong>{p.screenStatus === "flagged" ? "Kept after 3 screening attempts" : "Passed screening"}</strong> — {p.screenReason}
        </div>
        <span className="mt-auto border-t border-[#efefea] pt-3 font-mono text-xs text-muted">Demographics: census sample · Profile: Gemini Flash</span>
      </aside>
    </div>
  );
}
