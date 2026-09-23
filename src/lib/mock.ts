/** In-memory backend for `pnpm dev` in a browser. Not used inside the desktop app. */
import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { CohortProgress } from "../types/gen/CohortProgress";
import type { CohortSummary } from "../types/gen/CohortSummary";
import type { Project } from "../types/gen/Project";
import type { RespondentCard } from "../types/gen/RespondentCard";
import type { RespondentDetail } from "../types/gen/RespondentDetail";
import type { RespondentPage } from "../types/gen/RespondentPage";
import type { SurveyInfo } from "../types/gen/SurveyInfo";

const FIRST = ["Maya", "Gerald", "Priya", "Tom", "Sofia", "Andre", "Dana", "Luis", "Heather", "Kayla", "Ethan", "Latoya"];
const LAST = ["Ortiz", "Price", "Nair", "Brennan", "Reyes", "Wallace", "Kowalski", "Ortega", "Cole", "Nguyen", "Park", "Brooks"];
const JOBS = ["Service", "Sales & office", "Management & professional", "Production & transport", "Not in labor force"];
const BIASES = [["Status quo", "Price anchoring"], ["Brand-loyal", "Social proof"], ["Convenience-first", "Early adopter"]];

let project: Project | null = null;
let cohort: Cohort | null = null;
let people: RespondentDetail[] = [];

function person(i: number): RespondentDetail {
  const name = `${FIRST[i % 12]} ${LAST[(i * 5) % 12]}`;
  return {
    id: i,
    ordinal: i,
    name,
    age: 18 + ((i * 7) % 60),
    gender: i % 2 ? "Female" : "Male",
    country: "US",
    region: ["Northeast", "South", "Midwest", "West"][i % 4],
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

export const mock = {
  saveSurveyInfo: async (projectId: number | null, info: SurveyInfo): Promise<Project> => {
    const now = new Date().toISOString();
    project = { id: projectId ?? 1, ...info, wizardStep: 1, createdAt: project?.createdAt ?? now, updatedAt: now };
    return project;
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
};
