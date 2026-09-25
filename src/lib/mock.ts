/** In-memory backend for `pnpm dev` in a browser. Not used inside the desktop app. */
import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { CohortProgress } from "../types/gen/CohortProgress";
import type { CohortSummary } from "../types/gen/CohortSummary";
import type { Project } from "../types/gen/Project";
import type { RespondentCard } from "../types/gen/RespondentCard";
import type { RespondentDetail } from "../types/gen/RespondentDetail";
import type { RespondentPage } from "../types/gen/RespondentPage";
import type { CostEstimate } from "../types/gen/CostEstimate";
import type { CrossTab } from "../types/gen/CrossTab";
import type { ExportFormat } from "../types/gen/ExportFormat";
import type { Critique } from "../types/gen/Critique";
import type { Question } from "../types/gen/Question";
import type { QuestionReport } from "../types/gen/QuestionReport";
import type { Report } from "../types/gen/Report";
import type { QuestionBody } from "../types/gen/QuestionBody";
import type { RunProgress } from "../types/gen/RunProgress";
import type { Settings } from "../types/gen/Settings";
import type { SimulationRun } from "../types/gen/SimulationRun";
import type { Survey } from "../types/gen/Survey";
import type { SurveyInfo } from "../types/gen/SurveyInfo";

const FIRST = ["Maya", "Gerald", "Priya", "Tom", "Sofia", "Andre", "Dana", "Luis", "Heather", "Kayla", "Ethan", "Latoya"];
const LAST = ["Ortiz", "Price", "Nair", "Brennan", "Reyes", "Wallace", "Kowalski", "Ortega", "Cole", "Nguyen", "Park", "Brooks"];
const JOBS = ["Service", "Sales & office", "Management & professional", "Production & transport", "Not in labour force"];
const BIASES = [["Status quo", "Price anchoring"], ["Brand-loyal", "Social proof"], ["Convenience-first", "Early adopter"]];

let project: Project | null = null;
let cohort: Cohort | null = null;
let people: RespondentDetail[] = [];
let settings: Settings = { flashModel: null, proModel: null, usageTier: "free", flashPrice: null, proPrice: null };

function person(i: number): RespondentDetail {
  const name = `${FIRST[i % 12]} ${LAST[(i * 5) % 12]}`;
  return {
    id: i,
    ordinal: i,
    name,
    age: 18 + ((i * 7) % 60),
    gender: i % 2 ? "Female" : "Male",
    country: "CA",
    region: ["Ontario", "Quebec", "British Columbia", "Prairies", "Atlantic"][i % 5],
    income: ["Under $50k", "$50k–$100k", "Over $100k"][i % 3],
    occupation: JOBS[i % 5],
    summary: `${name} is a sample persona from the browser preview. In the desktop app this text is written by Gemini from census-based demographics.`,
    values: ["Reliability", "Family"],
    habits: "Checks reviews before buying.",
    mediaHabits: "Podcasts and news apps.",
    brandLoyalties: "None in particular.",
    categoryAttitudes: "Practical.",
    priceSensitivity: 1 + (i % 5),
    biases: BIASES[i % 3],
    categoryFacts: [
      ["Upgrade trigger", ["Battery", "Camera", "Damage"][i % 3]],
      ["Current device age years", String(i % 5)],
    ],
    screenStatus: "passed",
    screenReason: "Preview data.",
  };
}

function start(config: CohortConfig, onProgress: (p: CohortProgress) => void, id: number): Cohort {
  cohort = { id, projectId: project?.id ?? 1, name: `Cohort v${id}`, status: "generating", config, error: null, createdAt: new Date().toISOString() };
  people = [];
  let done = 0;
  const timer = setInterval(() => {
    const next = Math.min(done + 8, config.size);
    for (let i = done + 1; i <= next; i++) people.push(person(i));
    done = next;
    onProgress({ done, total: config.size, replaced: Math.floor(done / 20) });
    if (done >= config.size && cohort) {
      cohort = { ...cohort, status: "ready" };
      clearInterval(timer);
    }
  }, 150);
  return cohort;
}

const opts = (labels: string[]) => labels.map((label, i) => ({ code: String.fromCharCode(65 + i), label }));
const DRAFT: [string, QuestionBody][] = [
  ["Q1_OWN", { text: "Which brand is your current smartphone?", questionType: "single_choice", options: opts(["[Brand A]", "[Brand B]", "[Brand C]", "Other"]), randomize: true, maxChoices: null, scale: null, numeric: null }],
  ["Q2_AGE", { text: "How long have you had your current phone?", questionType: "single_choice", options: opts(["Under 1 year", "1–2 years", "2–3 years", "Over 3 years"]), randomize: false, maxChoices: null, scale: null, numeric: null }],
  ["Q3_INTENT", { text: "How likely are you to buy a new phone in the next 12 months?", questionType: "likert", options: [], randomize: false, maxChoices: null, scale: { min: 1, max: 7, minLabel: "Not at all likely", maxLabel: "Extremely likely" }, numeric: null }],
  ["Q4_DRIVERS", { text: "Which of these would most make you upgrade?", questionType: "multi_choice", options: opts(["Battery life", "Camera", "Price drop", "Damage", "Carrier deal"]), randomize: true, maxChoices: 2, scale: null, numeric: null }],
  ["Q5_BUDGET", { text: "What is the most you would pay for your next phone?", questionType: "numeric", options: [], randomize: false, maxChoices: null, scale: null, numeric: { min: 0, max: 2500, unit: "CAD" } }],
  ["Q6_WHY", { text: "In a sentence, what would stop you from upgrading?", questionType: "open_ended", options: [], randomize: false, maxChoices: null, scale: null, numeric: null }],
];
const SUGGESTED: [string, QuestionBody][] = [
  ["S1_TRADEIN", { text: "Would a trade-in offer change when you upgrade?", questionType: "single_choice", options: opts(["Yes, sooner", "No difference", "Not sure"]), randomize: true, maxChoices: null, scale: null, numeric: null }],
  ["S2_CARRIER", { text: "How satisfied are you with your mobile carrier?", questionType: "likert", options: [], randomize: false, maxChoices: null, scale: { min: 1, max: 5, minLabel: "Very dissatisfied", maxLabel: "Very satisfied" }, numeric: null }],
];

/** What the mock's "Suggest more" hands out, a few at a time, skipping any already present. */
const MORE: [string, QuestionBody][] = [
  ["S3_PAY", { text: "How do you usually pay for a new phone?", questionType: "single_choice", options: opts(["Outright", "Carrier plan", "Financing", "Other"]), randomize: true, maxChoices: null, scale: null, numeric: null }],
  ["S4_REFURB", { text: "Would you consider a refurbished phone?", questionType: "likert", options: [], randomize: false, maxChoices: null, scale: { min: 1, max: 5, minLabel: "Definitely not", maxLabel: "Definitely" }, numeric: null }],
  ["S5_SOURCE", { text: "Where do you look for information before buying a phone?", questionType: "multi_choice", options: opts(["Reviews", "Friends", "Store staff", "Social media"]), randomize: true, maxChoices: 2, scale: null, numeric: null }],
  ["S6_KEEP", { text: "How many years do you expect to keep your next phone?", questionType: "numeric", options: [], randomize: false, maxChoices: null, scale: null, numeric: { min: 0, max: 10, unit: "years" } }],
  ["S7_TRADEIN", { text: "Would a trade-in offer change when you upgrade?", questionType: "single_choice", options: opts(["Yes, sooner", "No difference", "Not sure"]), randomize: true, maxChoices: null, scale: null, numeric: null }],
];

/** Cost at the Flash price saved in Settings, like survey-core's `pricing`; null without one. */
const costOf = (input: number, output: number) => {
  const p = settings.flashPrice;
  return p ? (input * p.inputUsdPerMillion + output * p.outputUsdPerMillion) / 1e6 : null;
};
let runCost: number | null = null;

let survey: Survey | null = null;
let nextId = 100;
let run: SimulationRun | null = null;
let runTimer: ReturnType<typeof setInterval> | null = null;
let runListener: ((p: RunProgress) => void) | null = null;

/** Stand-in for the Gemini critic: flags obvious leading or double-barrelled wording. */
function mockCritique(b: QuestionBody): Critique {
  const flags: Critique["flags"] = [];
  if (/\b(love|amazing|great|don't you|wouldn't you)\b/i.test(b.text))
    flags.push({ issue: "leading", note: "The wording suggests the answer. Ask neutrally, e.g. \"How would you rate…\"." });
  if (/\b(and|or)\b/i.test(b.text.replace(/\?.*$/, "")) && b.questionType !== "multi_choice")
    flags.push({ issue: "double_barrelled", note: "This may ask about two things at once. Split it into two questions if so." });
  if (b.text.trim().split(/\s+/).length < 4) flags.push({ issue: "unclear", note: "Too short to be clear to a respondent. Say exactly what is being asked." });
  return { status: "done", flags, error: null, promptVersion: "critic.v1" };
}

/** Marks a question as being checked and fills in the result shortly after, like the real job. */
function checkLater(x: Question): Question {
  const body = x.body;
  setTimeout(() => {
    const s = survey;
    if (!s) return;
    const done = (y: Question) => (y.id === x.id && y.body === body ? { ...y, critique: mockCritique(body) } : y);
    s.questions = s.questions.map(done);
    s.suggestions = s.suggestions.map(done);
  }, 900);
  return { ...x, critique: { status: "checking", flags: [], error: null, promptVersion: "critic.v1" } };
}

function q(code: string, body: QuestionBody, active: boolean): Question {
  const id = nextId++;
  return { id, code, orderIndex: id, body, isActive: active, origin: "ai", reviewStatus: active ? "pending" : "suggested", objective: "Purchase intent and drivers", rationale: "Preview question.", critique: mockCritique(body) };
}

function ensureSurvey(): Survey {
  if (!survey) {
    survey = { id: 1, projectId: project?.id ?? 1, title: project?.title ?? "Survey", intro: "", status: "draft", draftStatus: "generating", draftError: null, questions: [], suggestions: [] };
    setTimeout(() => {
      if (!survey) return;
      survey = {
        ...survey,
        status: "in_review",
        draftStatus: "ready",
        intro: "Thanks for taking part. There are no right or wrong answers.",
        questions: DRAFT.map(([c, b]) => q(c, b, true)),
        suggestions: SUGGESTED.map(([c, b]) => q(c, b, false)),
      };
    }, 1500);
  }
  return survey;
}

function patchQuestion(id: number, f: (q: Question) => Question): Question {
  const s = ensureSurvey();
  s.questions = s.questions.map((x) => (x.id === id ? f(x) : x));
  if (s.status === "approved") s.status = "in_review";
  return s.questions.find((x) => x.id === id)!;
}

function tickRun(onProgress: (p: RunProgress) => void) {
  const s = ensureSurvey();
  const total = people.length * s.questions.length;
  runListener = onProgress;
  onProgress({ kind: "status", status: "running" });
  runTimer = setInterval(() => {
    if (!run) return;
    const start = run.respondentsDone;
    const end = Math.min(start + 3, people.length);
    const deltas = [];
    const consoleLines = [];
    for (let i = start; i < end; i++) {
      const p = people[i];
      for (const qq of s.questions) {
        const b = qq.body;
        const pick = b.options.length ? b.options[(i * 7 + qq.id) % b.options.length] : null;
        const value = b.scale ? b.scale.min + ((i + qq.id) % (b.scale.max - b.scale.min + 1)) : b.numeric ? 400 + ((i * 97) % 900) : null;
        deltas.push({ questionId: qq.id, respondentId: p.id, code: b.questionType === "single_choice" ? pick!.code : null, codes: b.questionType === "multi_choice" ? [pick!.code] : null, value });
        consoleLines.push({ at: String(Date.now()), respondent: p.ordinal, question: qq.code, answer: pick?.label ?? (value != null ? String(value) : "Only if my phone breaks."), reason: "Preview answer." });
      }
    }
    const batch = costOf(2600 * (end - start), (300 + 45 * s.questions.length) * (end - start));
    if (batch != null) runCost = (runCost ?? 0) + batch;
    run = { ...run, respondentsDone: end, answered: end * s.questions.length, costUsd: runCost };
    onProgress({ kind: "batch", answered: run.answered, totalAnswers: total, respondentsDone: end, costUsd: runCost, avgLatencyMs: 4200, p95LatencyMs: 7900, answersPerMin: 12 * s.questions.length * 3, concurrency: 4, deltas, console: consoleLines.slice(-20) });
    if (end >= people.length) {
      clearInterval(runTimer!);
      run = { ...run, status: "completed" };
      onProgress({ kind: "status", status: "completed" });
      setTimeout(() => { synthesisReady = true; }, 1500);
    }
  }, 250);
}

let synthesisReady = false;

/** Deterministic fake counts that sum to n. */
function split(n: number, k: number, salt: number): number[] {
  const w = Array.from({ length: k }, (_, i) => ((i + 1) * 37 + salt * 11) % 17 + 3);
  const total = w.reduce((a, b) => a + b, 0);
  const out = w.map((x) => Math.floor((x / total) * n));
  out[0] += n - out.reduce((a, b) => a + b, 0);
  return out;
}
const pct = (c: number, n: number) => (n ? Math.round((c / n) * 1000) / 10 : 0);

function questionReport(q: Question, n: number): QuestionReport {
  const b = q.body;
  const base: QuestionReport = {
    questionId: q.id, code: q.code, text: b.text, questionType: b.questionType, chart: "bar", n, invalid: 0, refused: 0,
    rows: [], mean: null, median: null, q1: null, q3: null, unit: null, themes: [], sampleAnswers: [],
    validity: { entropy: null, midpointRate: null, firstPositionRate: null, lastPositionRate: null, flags: [] },
  };
  if (b.questionType === "single_choice" || b.questionType === "multi_choice") {
    const counts = b.questionType === "multi_choice" ? b.options.map((_, i) => Math.round(n * (0.7 - i * 0.12))) : split(n, b.options.length, q.id);
    return { ...base, chart: b.questionType === "multi_choice" ? "multi_bar" : b.options.length <= 6 ? "pie" : "bar", rows: b.options.map((o, i) => ({ key: o.code, label: o.label, count: counts[i], percent: pct(counts[i], n), avgProb: null })) };
  }
  if (b.questionType === "likert" && b.scale) {
    const k = b.scale.max - b.scale.min + 1;
    const counts = split(n, k, q.id);
    const rows = counts.map((c, i) => {
      const v = b.scale!.min + i;
      const label = v === b.scale!.min ? `${v} – ${b.scale!.minLabel}` : v === b.scale!.max ? `${v} – ${b.scale!.maxLabel}` : String(v);
      return { key: String(v), label, count: c, percent: pct(c, n), avgProb: null };
    });
    const mean = Math.round((counts.reduce((a, c, i) => a + c * (b.scale!.min + i), 0) / n) * 100) / 100;
    const mid = k % 2 ? rows[(k - 1) / 2].percent : null;
    return { ...base, chart: "diverging", rows, mean, validity: { ...base.validity, entropy: 0.93, midpointRate: mid, flags: mid != null && mid > 60 ? [`Most respondents chose the midpoint (${mid}%), a common sign of model hedging.`] : [] } };
  }
  if (b.questionType === "numeric" && b.numeric) {
    const counts = split(n, 8, q.id);
    const w = (b.numeric.max - b.numeric.min) / 8;
    return {
      ...base, chart: "histogram", unit: b.numeric.unit || null, median: 900, q1: 600, q3: 1200, mean: 935,
      rows: counts.map((c, i) => ({ key: String(b.numeric!.min + i * w), label: `${b.numeric!.min + i * w}–${b.numeric!.min + (i + 1) * w}`, count: c, percent: pct(c, n), avgProb: null })),
    };
  }
  const t = [["Price is too high", 0.46], ["Current phone still works", 0.31], ["Waiting for a deal", 0.14]] as const;
  return {
    ...base, chart: "themes",
    themes: t.map(([label, share], i) => ({ id: i + 1, label, description: "Preview theme.", count: Math.round(n * share), percent: Math.round(share * 1000) / 10, quotes: ["Only if my phone breaks.", "They cost too much now."] })),
  };
}

export const mock = {
  saveSurveyInfo: async (projectId: number | null, info: SurveyInfo): Promise<Project> => {
    const now = new Date().toISOString();
    project = { id: projectId ?? 1, ...info, wizardStep: 1, createdAt: project?.createdAt ?? now, updatedAt: now };
    return project;
  },
  getLastProject: async (): Promise<Project | null> => project,
  getSettings: async (): Promise<Settings> => settings,
  saveSettings: async (s: Settings): Promise<Settings> => {
    settings = s;
    return settings;
  },
  generateCohort: async (_projectId: number, config: CohortConfig, onProgress: (p: CohortProgress) => void) =>
    start(config, onProgress, 1),
  regenerateCohort: async (cohortId: number, onProgress: (p: CohortProgress) => void) =>
    start(cohort!.config, onProgress, cohortId + 1),
  getLatestCohort: async () => cohort,
  getCohortSummary: async (): Promise<CohortSummary> => {
    const n = people.length;
    const f = people.filter((p) => p.gender === "Female").length;
    return {
      status: cohort?.status ?? "draft",
      respondents: n,
      averageAge: n ? Math.round((people.reduce((a, p) => a + p.age, 0) / n) * 10) / 10 : null,
      gender: n ? [{ label: "Female", percent: Math.round((f / n) * 1000) / 10 }, { label: "Male", percent: Math.round(((n - f) / n) * 1000) / 10 }] : [],
      topTrigger: n ? { label: "Battery", percent: 34 } : null,
      topTriggerLabel: "Upgrade trigger",
      replacedAtScreening: Math.floor(n / 20),
    };
  },
  listRespondents: async (query: string, offset: number, limit: number): Promise<RespondentPage> => {
    const q = query.toLowerCase();
    const hits = people.filter((p) => !q || `${p.name} ${p.occupation} ${p.biases.join(" ")}`.toLowerCase().includes(q));
    const items: RespondentCard[] = hits.slice(offset, offset + limit).map((p) => ({
      id: p.id, ordinal: p.ordinal, name: p.name, age: p.age, gender: p.gender, occupation: p.occupation, biases: p.biases,
    }));
    return { items, total: hits.length };
  },
  getRespondent: async (id: number) => people.find((p) => p.id === id)!,
  lockCohort: async (): Promise<Cohort> => {
    cohort = { ...cohort!, status: "locked" };
    return cohort;
  },
  getSurvey: async (): Promise<Survey> => structuredClone(ensureSurvey()),
  redraftSurvey: async (): Promise<Survey> => {
    const s = ensureSurvey();
    s.questions = s.questions.filter((x) => x.origin !== "ai" || x.reviewStatus !== "pending");
    s.suggestions = [];
    survey = null;
    const fresh = ensureSurvey();
    fresh.questions = s.questions;
    return structuredClone(fresh);
  },
  updateSurveyText: async (title: string, intro: string): Promise<Survey> => {
    const s = ensureSurvey();
    if (!title.trim()) throw { code: "invalid_input", message: "the survey needs a title" };
    if (s.intro !== intro.trim() && s.status === "approved") s.status = "in_review";
    s.title = title.trim();
    s.intro = intro.trim();
    return structuredClone(s);
  },
  updateQuestion: async (id: number, body: QuestionBody): Promise<Question> =>
    structuredClone(patchQuestion(id, (x) => checkLater({ ...x, body, origin: x.origin === "ai" ? "ai_edited" : x.origin, reviewStatus: "pending" }))),
  critiqueQuestion: async (id: number): Promise<Question> => structuredClone(patchQuestion(id, checkLater)),
  reorderQuestions: async (ids: number[]): Promise<Survey> => {
    const s = ensureSurvey();
    s.questions = ids.map((id, i) => ({ ...s.questions.find((x) => x.id === id)!, orderIndex: i + 1 }));
    return structuredClone(s);
  },
  addQuestion: async (): Promise<Question> => {
    const s = ensureSurvey();
    const nq: Question = { ...q(`Q${s.questions.length + 1}`, { text: "New question", questionType: "single_choice", options: opts(["Option 1", "Option 2"]), randomize: true, maxChoices: null, scale: null, numeric: null }, true), origin: "human", objective: null, rationale: null, critique: null };
    s.questions.push(nq);
    return structuredClone(nq);
  },
  deleteQuestion: async (id: number): Promise<Survey> => {
    const s = ensureSurvey();
    s.questions = s.questions.filter((x) => x.id !== id);
    return structuredClone(s);
  },
  approveQuestion: async (id: number): Promise<Question> => structuredClone(patchQuestion(id, (x) => ({ ...x, reviewStatus: "accepted" }))),
  addSuggestion: async (id: number): Promise<Survey> => {
    const s = ensureSurvey();
    const sug = s.suggestions.find((x) => x.id === id)!;
    s.suggestions = s.suggestions.filter((x) => x.id !== id);
    s.questions.push({ ...sug, isActive: true, reviewStatus: "pending" });
    return structuredClone(s);
  },
  suggestMore: async (): Promise<Survey> => {
    const s = ensureSurvey();
    await new Promise((r) => setTimeout(r, 600));
    const words = (t: string) => t.toLowerCase().replace(/[^a-z0-9 ]/g, "").trim();
    const have = new Set([...s.questions, ...s.suggestions].map((x) => words(x.body.text)));
    const fresh = MORE.filter(([, b]) => !have.has(words(b.text))).slice(0, 3);
    s.suggestions.push(...fresh.map(([c, b]) => checkLater(q(c, b, false))));
    return structuredClone(s);
  },
  estimateRun: async (): Promise<CostEstimate> => {
    const s = ensureSurvey();
    const calls = people.length;
    const inputTokens = calls * 2600;
    const outputTokens = calls * (300 + s.questions.reduce((a, x) => a + (x.body.questionType === "open_ended" ? 100 : 45), 0));
    return { model: "gemini-mock-flash", calls, inputTokens, outputTokens, costUsd: costOf(inputTokens, outputTokens), outputFromHistory: false };
  },
  startSimulation: async (onProgress: (p: RunProgress) => void): Promise<SimulationRun> => {
    const s = ensureSurvey();
    if (s.questions.some((x) => x.reviewStatus !== "accepted")) throw { code: "survey_not_approved", message: "Every question must be approved first." };
    s.status = "approved";
    runCost = settings.flashPrice ? 0 : null;
    const est = await mock.estimateRun();
    run = { id: (run?.id ?? 0) + 1, projectId: s.projectId, surveyId: s.id, cohortId: cohort?.id ?? 1, status: "running", model: "gemini-mock-flash", promptVersion: "answer.v2", respondents: people.length, questions: s.questions.length, answered: 0, respondentsDone: 0, error: null, createdAt: new Date().toISOString(), estCostUsd: est.costUsd, costUsd: runCost };
    tickRun(onProgress);
    return structuredClone(run);
  },
  getLatestRun: async () => (run ? structuredClone(run) : null),
  pauseRun: async () => {
    if (runTimer) clearInterval(runTimer);
    if (run) run = { ...run, status: "paused" };
    runListener?.({ kind: "status", status: "paused" });
  },
  resumeRun: async (onProgress: (p: RunProgress) => void): Promise<SimulationRun> => {
    run = { ...run!, status: "running" };
    tickRun(onProgress);
    return structuredClone(run);
  },
  stopRun: async (): Promise<SimulationRun> => {
    if (runTimer) clearInterval(runTimer);
    run = { ...run!, status: "stopped" };
    runListener?.({ kind: "status", status: "stopped" });
    setTimeout(() => { synthesisReady = true; }, 1500);
    return structuredClone(run);
  },
  getReport: async (): Promise<Report> => {
    const s = ensureSurvey();
    const n = run?.respondentsDone ?? 0;
    return {
      run: structuredClone(run!),
      basedOnN: n,
      questions: s.questions.map((q) => questionReport(q, n)),
      dimensions: [{ key: "age", label: "Age" }, { key: "gender", label: "Gender" }, { key: "region", label: "Region" }, { key: "income", label: "Household income" }],
      synthesis: synthesisReady
        ? {
            summary: "Most respondents keep their phone until it fails, and price is the main reason to wait. Intent to buy in the next year is mixed.",
            frictionPoints: [{ label: "Price is too high", mentions: Math.round(n * 0.46) }, { label: "Current phone still works", mentions: Math.round(n * 0.31) }],
            segments: [{ dimension: "age", group: "18–29", takeaway: "Younger respondents mention camera upgrades more often. (low base, n=6)" }],
            basedOnN: n, model: "gemini-mock-pro", dropped: 1, createdAt: new Date().toISOString(),
          }
        : null,
      synthesisStatus: synthesisReady ? "ready" : "generating",
      synthesisError: null,
    };
  },
  getCrosstab: async (questionId: number, dimension: string): Promise<CrossTab> => {
    const s = ensureSurvey();
    const q = s.questions.find((x) => x.id === questionId)!;
    const n = run?.respondentsDone ?? 0;
    const r = questionReport(q, n);
    const cols = r.themes.length ? r.themes.map((t) => ({ key: String(t.id), label: t.label, count: t.count, percent: t.percent, avgProb: null })) : r.chart === "histogram" ? [] : r.rows;
    const groups = { age: ["18–29", "30–44", "45–59", "60+"], gender: ["Female", "Male"], region: ["Ontario", "Quebec", "British Columbia", "Prairies", "Atlantic"], income: ["Under $50k", "$50k–$100k", "Over $100k"] }[dimension] ?? ["All"];
    return {
      questionId, dimension: { key: dimension, label: dimension }, columns: cols,
      groups: groups.map((label, gi) => {
        const gn = Math.max(1, Math.round(n / groups.length));
        const counts = split(gn, Math.max(1, cols.length), gi + questionId);
        return { label, n: gn, lowBase: gn < 30, cells: cols.map((_, i) => pct(counts[i], gn)), mean: r.mean != null ? Math.round((r.mean + (gi - 1) * 0.3) * 100) / 100 : null };
      }),
    };
  },
  regenerateSynthesis: async (): Promise<Report> => {
    synthesisReady = false;
    setTimeout(() => { synthesisReady = true; }, 1200);
    return mock.getReport();
  },
  exportRun: async (format: ExportFormat): Promise<string | null> => `Downloads/preview-export.${format}`,
};
