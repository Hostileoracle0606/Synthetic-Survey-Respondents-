import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage } from "../../lib/api";
import { useWizard } from "../../store/wizard";
import { useRun } from "../../store/run";
import type { CriticIssue } from "../../types/gen/CriticIssue";
import type { Critique } from "../../types/gen/Critique";
import type { Question } from "../../types/gen/Question";
import type { QuestionBody } from "../../types/gen/QuestionBody";
import type { QuestionType } from "../../types/gen/QuestionType";
import type { Survey } from "../../types/gen/Survey";
import { AppShell } from "../../components/AppShell";
import { Arrow, pillButton, primaryButton } from "../../components/fields";
import { moveBefore, sameOrder } from "../../lib/reorder";
import { tokens, usd } from "../../lib/cost";
import type { CostEstimate } from "../../types/gen/CostEstimate";

const TYPES: [QuestionType, string][] = [
  ["single_choice", "Single choice"],
  ["multi_choice", "Multiple choice"],
  ["likert", "Scale"],
  ["numeric", "Number"],
  ["open_ended", "Open answer"],
];
const typeLabel = (t: QuestionType) => TYPES.find(([k]) => k === t)?.[1] ?? t;
const ISSUES: Record<CriticIssue, string> = { leading: "Leading", double_barrelled: "Double-barrelled", unclear: "Unclear" };

/** One line for a question card: what the critic found, or that it is still checking. */
function critiqueSummary(c: Critique | null): { text: string; warn: boolean } | null {
  if (!c) return null;
  if (c.status === "checking") return { text: "Checking wording…", warn: false };
  if (c.status === "failed") return { text: "Wording check failed", warn: false };
  if (c.flags.length === 0) return null;
  return { text: `⚑ ${c.flags.map((f) => ISSUES[f.issue]).join(", ")}`, warn: true };
}

/** The critic's flags on the selected question. Advice only: approval never waits on it. */
function CritiquePanel({ critique, busy, onCheck }: { critique: Critique | null; busy: boolean; onCheck: () => void }) {
  const again = <button type="button" className={`${smallButton} h-8 shrink-0`} onClick={onCheck} disabled={busy}>Check again</button>;
  if (!critique) {
    return (
      <div className="flex items-center justify-between gap-3 rounded-2xl border border-line px-4 py-3 text-sm text-muted">
        <span>The wording is checked by Gemini when you save the question.</span>
        {again}
      </div>
    );
  }
  if (critique.status === "checking") {
    return <p className="m-0 rounded-2xl border border-line px-4 py-3 text-sm text-muted" role="status">Gemini is checking the wording for leading, double-barrelled or unclear questions…</p>;
  }
  if (critique.status === "failed") {
    return (
      <div className="flex items-center justify-between gap-3 rounded-2xl border border-line px-4 py-3 text-sm text-muted">
        <span>The wording check didn't finish{critique.error ? `: ${critique.error}` : ""}.</span>
        {again}
      </div>
    );
  }
  if (critique.flags.length === 0) {
    return <p className="m-0 rounded-2xl border border-[#cfe0c4] bg-[#f1f7ec] px-4 py-3 text-sm text-[#3d5a2a]">No wording problems found.</p>;
  }
  return (
    <div className="flex flex-col gap-2 rounded-2xl border border-[#ecd9ae] bg-[#fbf5e8] px-4 py-3 text-sm text-[#6b4f16]" aria-label="Wording flags">
      {critique.flags.map((f) => (
        <p key={f.issue} className="m-0"><strong className="font-medium">{ISSUES[f.issue]}:</strong> {f.note}</p>
      ))}
      <p className="m-0 text-xs">Advice from Gemini's critic. Fix the wording or approve it as it is.</p>
    </div>
  );
}

const smallButton =
  "inline-flex h-9 items-center justify-center rounded-full border border-line px-3.5 text-sm hover:bg-[#f1f1ee] disabled:cursor-not-allowed disabled:opacity-40";
const input =
  "h-11 w-full rounded-2xl border border-line bg-white px-4 text-[15px] focus:border-accent focus:outline-none focus:ring-4 focus:ring-accent/15";

/** Fills in the fields a type needs when the reviewer switches type. */
function withType(b: QuestionBody, t: QuestionType): QuestionBody {
  const choice = t === "single_choice" || t === "multi_choice";
  return {
    ...b,
    questionType: t,
    options: choice && b.options.length < 2 ? [{ code: "A", label: "Option 1" }, { code: "B", label: "Option 2" }] : b.options,
    randomize: choice ? b.randomize : false,
    maxChoices: t === "multi_choice" ? b.maxChoices ?? Math.max(1, b.options.length) : null,
    scale: t === "likert" ? b.scale ?? { min: 1, max: 5, minLabel: "", maxLabel: "" } : null,
    numeric: t === "numeric" ? b.numeric ?? { min: 0, max: 100, unit: "" } : null,
  };
}

function nextCode(b: QuestionBody): string {
  const used = new Set(b.options.map((o) => o.code));
  for (let i = 0; ; i++) {
    const c = i < 26 ? String.fromCharCode(65 + i) : `O${i}`;
    if (!used.has(c)) return c;
  }
}

/** Survey title (for the researcher) and the intro respondents read before Q1. */
function SurveyText({ survey, busy, onSave }: { survey: Survey; busy: boolean; onSave: (title: string, intro: string) => void }) {
  const [title, setTitle] = useState(survey.title);
  const [intro, setIntro] = useState(survey.intro);
  // Follow the saved text (e.g. when the draft arrives) unless the reviewer is mid-edit.
  const saved = useRef({ title: survey.title, intro: survey.intro });
  useEffect(() => {
    setTitle((t) => (t === saved.current.title ? survey.title : t));
    setIntro((t) => (t === saved.current.intro ? survey.intro : t));
    saved.current = { title: survey.title, intro: survey.intro };
  }, [survey.title, survey.intro]);
  const dirty = title !== survey.title || intro !== survey.intro;
  return (
    <section aria-label="Survey title and intro" className="grid grid-cols-[300px_1fr_auto] items-start gap-6 rounded-[26px] border border-line bg-white p-5">
      <div className="flex flex-col gap-2">
        <label htmlFor="stitle" className="font-display text-[15px] font-medium">Survey title</label>
        <input id="stitle" className={input} value={title} maxLength={200} onChange={(e) => setTitle(e.target.value)} />
        <p className="m-0 text-xs text-muted">For you; respondents don't see it.</p>
      </div>
      <div className="flex flex-col gap-2">
        <label htmlFor="sintro" className="font-display text-[15px] font-medium">Intro shown to respondents</label>
        <textarea id="sintro" rows={2} className={`${input} h-auto py-3`} value={intro} maxLength={2000} placeholder="Shown before the first question. Don't reveal the research objective." onChange={(e) => setIntro(e.target.value)} />
      </div>
      <button type="button" className={`${pillButton} mt-7`} disabled={!dirty || busy || !title.trim()} onClick={() => onSave(title, intro)}>Save</button>
    </section>
  );
}

/** Step 3 (docs/DATA_FLOW.md §3, Step 3): review every drafted question before the run. */
export function QuestionnaireStep() {
  const { info, projectId, goTo, reach } = useWizard();
  const run = useRun();
  const [survey, setSurvey] = useState<Survey | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [draft, setDraft] = useState<QuestionBody | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const listRef = useRef<HTMLOListElement>(null);

  const load = useCallback(async () => {
    if (projectId == null) return;
    try {
      const s = await api.getSurvey(projectId);
      setSurvey(s);
      setSelected((cur) => (cur != null && s.questions.some((q) => q.id === cur) ? cur : s.questions[0]?.id ?? null));
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [projectId]);

  useEffect(() => {
    load();
  }, [load]);
  const generating = survey?.draftStatus === "generating";
  // Critic results arrive in the background, one question at a time.
  const checking = !!survey && [...survey.questions, ...survey.suggestions].some((q) => q.critique?.status === "checking");
  useEffect(() => {
    if (!generating && !checking) return;
    const t = setInterval(load, 1500);
    return () => clearInterval(t);
  }, [generating, checking, load]);

  const current = survey?.questions.find((q) => q.id === selected) ?? null;
  useEffect(() => {
    setDraft(current ? structuredClone(current.body) : null);
  }, [current?.id, current?.body]); // eslint-disable-line react-hooks/exhaustive-deps
  const dirty = !!current && !!draft && JSON.stringify(draft) !== JSON.stringify(current.body);

  const approved = survey?.questions.filter((q) => q.reviewStatus === "accepted").length ?? 0;
  const total = survey?.questions.length ?? 0;
  const canRun = total > 0 && approved === total && !dirty && !generating;

  function replace(q: Question) {
    setSurvey((s) => (s ? { ...s, questions: s.questions.map((x) => (x.id === q.id ? q : x)) } : s));
  }

  async function act<T>(f: () => Promise<T>): Promise<T | undefined> {
    setError("");
    setBusy(true);
    try {
      return await f();
    } catch (e) {
      setError(errorMessage(e));
      return undefined;
    } finally {
      setBusy(false);
    }
  }

  const save = () => act(async () => {
    if (!current || !draft) return;
    const q = await api.updateQuestion(current.id, draft);
    replace(q);
    return q;
  });
  const approve = () => act(async () => {
    if (!current) return;
    let q = current;
    if (dirty && draft) q = await api.updateQuestion(current.id, draft);
    replace(await api.approveQuestion(q.id));
  });
  const checkAgain = () => act(async () => {
    if (!current) return;
    replace(await api.critiqueQuestion(current.id));
  });
  const addNew = () => act(async () => {
    if (!survey) return;
    const q = await api.addQuestion(survey.id);
    setSurvey({ ...survey, questions: [...survey.questions, q] });
    setSelected(q.id);
  });
  const remove = () => act(async () => {
    if (!current) return;
    setSurvey(await api.deleteQuestion(current.id));
    setSelected(null);
  });
  const addSuggestion = (id: number) => act(async () => {
    const s = await api.addSuggestion(id);
    setSurvey(s);
    setSelected(id);
  });
  const saveText = (title: string, intro: string) => act(async () => {
    if (!survey) return;
    setSurvey(await api.updateSurveyText(survey.id, title, intro));
  });
  const [suggesting, setSuggesting] = useState(false);
  const [suggestNote, setSuggestNote] = useState("");
  const suggestMore = async () => {
    if (!survey) return;
    setSuggestNote("");
    setSuggesting(true);
    const before = survey.suggestions.length;
    const s = await act(() => api.suggestMore(survey.id));
    setSuggesting(false);
    if (!s) return;
    setSurvey(s);
    const added = s.suggestions.length - before;
    setSuggestNote(added > 0 ? `${added} new suggestion${added === 1 ? "" : "s"} added.` : "Gemini only repeated questions you already have. Try again later, or write your own with + New.");
  };
  const redraft = () => act(async () => {
    if (projectId == null) return;
    setSurvey(await api.redraftSurvey(projectId));
  });
  /** Saves a new order (same `reorder_questions` call for buttons, keys and drag) and keeps focus on the moved question. */
  const reorder = (ids: number[], focus: number) => act(async () => {
    if (!survey || sameOrder(ids, survey.questions.map((q) => q.id))) return;
    setSurvey(await api.reorderQuestions(survey.id, ids));
    requestAnimationFrame(() => listRef.current?.querySelector<HTMLButtonElement>(`[data-q="${focus}"]`)?.focus());
  });
  const move = (id: number, by: -1 | 1) => {
    if (!survey) return;
    const ids = survey.questions.map((q) => q.id);
    const i = ids.indexOf(id);
    const j = i + by;
    if (j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    return reorder(ids, id);
  };
  // Drag and drop: `dropBefore` is the question the dragged one lands in front of (null = the end).
  const [dragging, setDragging] = useState<number | null>(null);
  const [dropBefore, setDropBefore] = useState<number | null | undefined>(undefined);
  const endDrag = () => {
    setDragging(null);
    setDropBefore(undefined);
  };
  const drop = () => {
    if (survey && dragging != null && dropBefore !== undefined) {
      reorder(moveBefore(survey.questions.map((q) => q.id), dragging, dropBefore), dragging);
    }
    endDrag();
  };
  // Distribution mode (SPEC §8, BACKLOG B23): hidden until the answering model is confirmed
  // to return log-probabilities.
  const [logprobsSupported, setLogprobsSupported] = useState(false);
  const [logprobs, setLogprobs] = useState(false);
  useEffect(() => {
    let live = true;
    api.probeLogprobs().then((ok) => live && setLogprobsSupported(ok)).catch(() => live && setLogprobsSupported(false));
    return () => {
      live = false;
    };
  }, []);

  const startRun = () => act(async () => {
    if (projectId == null) return;
    run.reset();
    const r = await api.startSimulation(projectId, { seed: null, logprobs: logprobsSupported && logprobs }, run.apply);
    run.setRun(r);
    reach(3);
  });

  // Cost of the run the button would start; refreshed when the questions change.
  const [estimate, setEstimate] = useState<CostEstimate | null>(null);
  const questionKey = survey ? JSON.stringify([survey.intro, survey.questions.map((q) => [q.id, q.body])]) : "";
  useEffect(() => {
    if (projectId == null || !survey || survey.questions.length === 0) {
      setEstimate(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      api.estimateRun(projectId).then((e) => live && setEstimate(e)).catch(() => live && setEstimate(null));
    }, 400);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [projectId, questionKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const heading = useMemo(() => {
    if (generating) return "Gemini is drafting the questionnaire…";
    return `${approved} of ${total} approved`;
  }, [generating, approved, total]);

  return (
    <AppShell
      title={info.title || "Project"}
      heading="Questionnaire Review"
      subtitle="Gemini drafts the questions; you edit and approve each one. The simulation can only run on an approved survey."
      footer={
        <>
          <button type="button" className={pillButton} onClick={() => goTo(1)}>Back</button>
          <div className="flex items-center gap-4">
            <span className="text-sm text-muted" role="status">{heading}</span>
            {logprobsSupported && (
              <label className="flex items-center gap-2 text-sm text-muted" title="Record each option's probability for single-choice questions (one extra call per question, per respondent).">
                <input type="checkbox" checked={logprobs} onChange={(e) => setLogprobs(e.target.checked)} /> Distribution mode
              </label>
            )}
            {estimate && (
              <span
                className="text-sm text-muted"
                data-testid="cost-estimate"
                title={`${estimate.model}: ${tokens(estimate.inputTokens)} input and about ${tokens(estimate.outputTokens)} output tokens${estimate.outputFromHistory ? " (output from earlier runs)" : ""}. Cache hits make it cheaper.`}
              >
                Est. {estimate.costUsd != null ? `≈ ${usd(estimate.costUsd)}` : "cost needs the Flash price in Settings"} · {estimate.calls.toLocaleString()} calls
              </span>
            )}
            <button type="button" className={primaryButton} disabled={!canRun || busy} onClick={startRun} title={canRun ? "" : "Approve every question first"}>
              Run Survey Simulation <Arrow />
            </button>
          </div>
        </>
      }
    >
      {error && <div role="alert" className="rounded-2xl border border-[#e7b4a8] bg-[#fbeee9] px-5 py-3 text-[15px] text-[#8a3a26]">{error}</div>}
      {survey?.draftStatus === "failed" && (
        <div role="alert" className="flex items-center justify-between gap-4 rounded-2xl border border-[#e7b4a8] bg-[#fbeee9] px-5 py-3 text-[15px] text-[#8a3a26]">
          <span>Draft failed: {survey.draftError ?? "unknown error"}. You can retry or write questions by hand.</span>
          <button type="button" className={smallButton} onClick={redraft}>Retry draft</button>
        </div>
      )}

      {survey && <SurveyText survey={survey} busy={busy} onSave={saveText} />}

      <div className="grid min-h-[520px] grid-cols-[300px_1fr_280px] gap-6">
        {/* Question list */}
        <section aria-label="Questions" className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <h2 className="m-0 font-display text-lg font-semibold">Questions</h2>
            <div className="flex gap-2">
              <button type="button" className={smallButton} onClick={redraft} disabled={busy || generating} title="Replace AI questions nobody has edited or approved">Redraft</button>
              <button type="button" className={smallButton} onClick={addNew} disabled={busy || !survey}>+ New</button>
            </div>
          </div>
          {generating && <p className="m-0 text-sm text-muted" role="status">Drafting from your research brief. This takes a few seconds; you can add questions by hand meanwhile.</p>}
          <ol
            ref={listRef}
            className="m-0 flex list-none flex-col gap-2 p-0"
            onDragOver={(e) => {
              if (dragging == null) return;
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              // Below the last card: drop at the end. (Gaps between cards keep the last target.)
              const last = e.currentTarget.lastElementChild?.getBoundingClientRect();
              if (e.target === e.currentTarget && last && e.clientY > last.bottom) setDropBefore(null);
            }}
            onDrop={(e) => {
              e.preventDefault();
              drop();
            }}
          >
            {survey?.questions.map((q, i, all) => (
              <li
                key={q.id}
                draggable={!busy}
                onDragStart={(e) => {
                  e.dataTransfer.effectAllowed = "move";
                  e.dataTransfer.setData("text/plain", String(q.id));
                  setDragging(q.id);
                }}
                onDragEnd={endDrag}
                onDragOver={(e) => {
                  if (dragging == null) return;
                  e.preventDefault();
                  // Top half: in front of this card; bottom half: in front of the next one.
                  const r = e.currentTarget.getBoundingClientRect();
                  setDropBefore(e.clientY < r.top + r.height / 2 ? q.id : all[i + 1]?.id ?? null);
                }}
                className={`relative rounded-2xl ${dragging === q.id ? "opacity-40" : ""} ${dragging != null && dropBefore === q.id ? "before:absolute before:-top-1.5 before:left-2 before:right-2 before:h-0.5 before:rounded-full before:bg-accent" : ""} ${dragging != null && dropBefore === null && i === all.length - 1 ? "after:absolute after:-bottom-1.5 after:left-2 after:right-2 after:h-0.5 after:rounded-full after:bg-accent" : ""}`}
              >
                <button
                  type="button"
                  data-q={q.id}
                  aria-current={q.id === selected}
                  onClick={() => setSelected(q.id)}
                  onKeyDown={(e) => {
                    if (e.altKey && e.key === "ArrowUp") { e.preventDefault(); move(q.id, -1); }
                    if (e.altKey && e.key === "ArrowDown") { e.preventDefault(); move(q.id, 1); }
                  }}
                  title="Drag, or Alt+↑ / Alt+↓, to reorder"
                  className={`flex w-full cursor-grab flex-col gap-1 rounded-2xl border px-4 py-3 text-left active:cursor-grabbing ${q.id === selected ? "border-accent bg-white ring-4 ring-accent/10" : "border-line bg-white hover:border-[#bdbdb5]"}`}
                >
                  <span className="flex items-center justify-between gap-2 font-mono text-xs text-muted">
                    <span>{i + 1}. {q.code}</span>
                    <span className={q.reviewStatus === "accepted" ? "rounded-full bg-[#e6efe0] px-2 py-0.5 text-[#3d5a2a]" : "rounded-full bg-[#f6ecd9] px-2 py-0.5 text-[#7a5a1c]"}>
                      {q.reviewStatus === "accepted" ? "Approved" : "Needs review"}
                    </span>
                  </span>
                  <span className="line-clamp-2 text-[15px]">{q.body.text}</span>
                  <span className="text-xs text-muted">{typeLabel(q.body.questionType)}{q.origin === "human" ? " · written by you" : q.origin === "ai_edited" ? " · AI, edited" : " · AI draft"}</span>
                  {(() => {
                    const c = critiqueSummary(q.critique);
                    return c && <span className={`text-xs ${c.warn ? "text-[#8a5a00]" : "text-muted"}`}>{c.text}</span>;
                  })()}
                </button>
              </li>
            ))}
          </ol>
          {current && (
            <div className="flex gap-2">
              <button type="button" className={smallButton} onClick={() => move(current.id, -1)} disabled={busy} aria-label="Move up">↑ Up</button>
              <button type="button" className={smallButton} onClick={() => move(current.id, 1)} disabled={busy} aria-label="Move down">↓ Down</button>
            </div>
          )}
        </section>

        {/* Editor */}
        <section aria-label="Question editor" className="flex flex-col gap-4 rounded-[26px] border border-line bg-white p-6">
          {!current || !draft ? (
            <p className="m-auto text-muted">{generating ? "Questions will appear here as soon as the draft is ready." : "Select a question, or add one with + New."}</p>
          ) : (
            <>
              <div className="flex items-center gap-3">
                <label htmlFor="qtype" className="font-display text-[15px] font-medium">Type</label>
                <select id="qtype" className={`${input} w-56`} value={draft.questionType} onChange={(e) => setDraft(withType(draft, e.target.value as QuestionType))}>
                  {TYPES.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
                </select>
                <span className="ml-auto font-mono text-xs text-muted">{current.code}</span>
              </div>
              <label htmlFor="qtext" className="font-display text-[15px] font-medium">Question</label>
              <textarea id="qtext" rows={3} className={`${input} h-auto py-3`} value={draft.text} onChange={(e) => setDraft({ ...draft, text: e.target.value })} />

              {(draft.questionType === "single_choice" || draft.questionType === "multi_choice") && (
                <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
                  <legend className="mb-2 font-display text-[15px] font-medium">Options</legend>
                  {draft.options.map((o, i) => (
                    <div key={o.code} className="flex items-center gap-2">
                      <span className="w-6 font-mono text-xs text-muted">{o.code}</span>
                      <input className={input} value={o.label} aria-label={`Option ${o.code}`}
                        onChange={(e) => setDraft({ ...draft, options: draft.options.map((x, j) => (j === i ? { ...x, label: e.target.value } : x)) })} />
                      <button type="button" className={smallButton} aria-label={`Remove option ${o.code}`} disabled={draft.options.length <= 2}
                        onClick={() => setDraft({ ...draft, options: draft.options.filter((_, j) => j !== i) })}>✕</button>
                    </div>
                  ))}
                  <div className="flex flex-wrap items-center gap-4">
                    <button type="button" className={smallButton} disabled={draft.options.length >= 15}
                      onClick={() => setDraft({ ...draft, options: [...draft.options, { code: nextCode(draft), label: "" }] })}>+ Option</button>
                    <label className="flex items-center gap-2 text-sm">
                      <input type="checkbox" checked={draft.randomize} onChange={(e) => setDraft({ ...draft, randomize: e.target.checked })} />
                      Shuffle order for each respondent
                    </label>
                    {draft.questionType === "multi_choice" && (
                      <label className="flex items-center gap-2 text-sm">
                        Pick up to
                        <input type="number" min={1} max={draft.options.length} className={`${input} h-9 w-20`} value={draft.maxChoices ?? draft.options.length}
                          onChange={(e) => setDraft({ ...draft, maxChoices: Number(e.target.value) || 1 })} />
                      </label>
                    )}
                  </div>
                </fieldset>
              )}

              {draft.questionType === "likert" && draft.scale && (
                <div className="grid grid-cols-[100px_1fr] items-center gap-3 text-sm">
                  <label htmlFor="smin">From</label>
                  <div className="flex gap-2">
                    <input id="smin" type="number" className={`${input} w-24`} value={draft.scale.min} onChange={(e) => setDraft({ ...draft, scale: { ...draft.scale!, min: Number(e.target.value) } })} />
                    <input aria-label="Label for the lowest point" className={input} placeholder="e.g. Strongly disagree" value={draft.scale.minLabel} onChange={(e) => setDraft({ ...draft, scale: { ...draft.scale!, minLabel: e.target.value } })} />
                  </div>
                  <label htmlFor="smax">To</label>
                  <div className="flex gap-2">
                    <input id="smax" type="number" className={`${input} w-24`} value={draft.scale.max} onChange={(e) => setDraft({ ...draft, scale: { ...draft.scale!, max: Number(e.target.value) } })} />
                    <input aria-label="Label for the highest point" className={input} placeholder="e.g. Strongly agree" value={draft.scale.maxLabel} onChange={(e) => setDraft({ ...draft, scale: { ...draft.scale!, maxLabel: e.target.value } })} />
                  </div>
                </div>
              )}

              {draft.questionType === "numeric" && draft.numeric && (
                <div className="flex items-center gap-3 text-sm">
                  <label htmlFor="nmin">From</label>
                  <input id="nmin" type="number" className={`${input} w-32`} value={draft.numeric.min} onChange={(e) => setDraft({ ...draft, numeric: { ...draft.numeric!, min: Number(e.target.value) } })} />
                  <label htmlFor="nmax">to</label>
                  <input id="nmax" type="number" className={`${input} w-32`} value={draft.numeric.max} onChange={(e) => setDraft({ ...draft, numeric: { ...draft.numeric!, max: Number(e.target.value) } })} />
                  <label htmlFor="nunit">Unit</label>
                  <input id="nunit" className={`${input} w-28`} placeholder="CAD" value={draft.numeric.unit} onChange={(e) => setDraft({ ...draft, numeric: { ...draft.numeric!, unit: e.target.value } })} />
                </div>
              )}

              <CritiquePanel critique={current.critique} busy={busy} onCheck={checkAgain} />

              {(current.objective || current.rationale) && (
                <div className="rounded-2xl bg-[#f6f6f2] px-4 py-3 text-sm text-muted">
                  {current.objective && <p className="m-0"><strong className="font-medium text-ink">Serves:</strong> {current.objective}</p>}
                  {current.rationale && <p className="m-0 mt-1"><strong className="font-medium text-ink">Why ask:</strong> {current.rationale}</p>}
                  <p className="m-0 mt-1 text-xs">For reviewers only. Respondents never see this.</p>
                </div>
              )}

              <div className="mt-auto flex items-center justify-between gap-3 pt-2">
                <button type="button" className={smallButton} onClick={remove} disabled={busy}>Delete question</button>
                <div className="flex gap-3">
                  <button type="button" className={pillButton} onClick={save} disabled={!dirty || busy}>Save changes</button>
                  <button type="button" className={primaryButton} onClick={approve} disabled={busy || (!dirty && current.reviewStatus === "accepted")}>
                    {current.reviewStatus === "accepted" && !dirty ? "Approved ✓" : dirty ? "Save & approve" : "Approve question"}
                  </button>
                </div>
              </div>
            </>
          )}
        </section>

        {/* Suggestions */}
        <aside aria-label="AI suggestions" className="flex flex-col gap-3">
          <div className="flex items-center justify-between">
            <h2 className="m-0 font-display text-lg font-semibold">AI suggestions</h2>
            <button type="button" className={smallButton} onClick={suggestMore} disabled={busy || generating || !survey} title="Ask Gemini for more ideas; repeats of your questions are left out">
              {suggesting ? "Suggesting…" : "Suggest more"}
            </button>
          </div>
          <p className="m-0 text-sm text-muted">Optional extras from the draft. Added questions still need your approval.</p>
          {suggesting && <p className="m-0 text-sm text-muted" role="status">Gemini is writing new suggestions…</p>}
          {suggestNote && !suggesting && <p className="m-0 text-sm text-muted" role="status">{suggestNote}</p>}
          {survey?.suggestions.length === 0 && !generating && !suggesting && <p className="m-0 text-sm text-muted">No suggestions left.</p>}
          {survey?.suggestions.map((s) => (
            <div key={s.id} className="flex flex-col gap-2 rounded-2xl border border-line bg-white px-4 py-3">
              <span className="text-[15px]">{s.body.text}</span>
              <span className="text-xs text-muted">{typeLabel(s.body.questionType)}{s.body.options.length ? ` · ${s.body.options.length} options` : ""}</span>
              {(() => {
                const c = critiqueSummary(s.critique);
                return c && <span className={`text-xs ${c.warn ? "text-[#8a5a00]" : "text-muted"}`} title={s.critique?.flags.map((f) => `${ISSUES[f.issue]}: ${f.note}`).join("\n")}>{c.text}</span>;
              })()}
              <button type="button" className={`${smallButton} self-start`} onClick={() => addSuggestion(s.id)} disabled={busy}>+ Add to survey</button>
            </div>
          ))}
        </aside>
      </div>
    </AppShell>
  );
}
