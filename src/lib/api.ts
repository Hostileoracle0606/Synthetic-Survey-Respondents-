/**
 * Typed wrappers around Tauri commands (src-tauri/src/commands.rs). Types come from
 * src/types/gen, generated from survey-core by `cargo test`.
 *
 * Outside the desktop app (plain `pnpm dev` in a browser) an in-memory mock is used, so
 * screens can be worked on without the Rust side. The mock fakes persona generation.
 */
import { Channel, invoke } from "@tauri-apps/api/core";
import countries from "../../crates/core/data/countries.json";
import type { AppError } from "../types/gen/AppError";
import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { CohortProgress } from "../types/gen/CohortProgress";
import type { CohortSummary } from "../types/gen/CohortSummary";
import type { CountryOption } from "../types/gen/CountryOption";
import type { Project } from "../types/gen/Project";
import type { QuotaGroup } from "../types/gen/QuotaGroup";
import type { RespondentDetail } from "../types/gen/RespondentDetail";
import type { RespondentPage } from "../types/gen/RespondentPage";
import type { SurveyInfo } from "../types/gen/SurveyInfo";
import { defaultQuotaGroups } from "./quota";
import { mock } from "./mock";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
  return isAppError(e) ? e.message : String(e);
}

function progressChannel(onProgress: (p: CohortProgress) => void): Channel<CohortProgress> {
  const ch = new Channel<CohortProgress>();
  ch.onmessage = onProgress;
  return ch;
}

export const api = {
  listCountries: (): Promise<CountryOption[]> =>
    inTauri ? invoke("list_countries") : Promise.resolve(countries as CountryOption[]),
  defaultQuotas: (codes: string[]): Promise<QuotaGroup[]> =>
    inTauri
      ? invoke("default_quotas", { countries: codes })
      : Promise.resolve(defaultQuotaGroups((countries as CountryOption[]).filter((c) => codes.includes(c.code)))),
  saveSurveyInfo: (projectId: number | null, info: SurveyInfo): Promise<Project> =>
    inTauri ? invoke("save_survey_info", { projectId, info }) : mock.saveSurveyInfo(projectId, info),
  generateCohort: (projectId: number, config: CohortConfig, onProgress: (p: CohortProgress) => void): Promise<Cohort> =>
    inTauri
      ? invoke("generate_cohort", { projectId, config, onProgress: progressChannel(onProgress) })
      : mock.generateCohort(projectId, config, onProgress),
  regenerateCohort: (cohortId: number, onProgress: (p: CohortProgress) => void): Promise<Cohort> =>
    inTauri
      ? invoke("regenerate_cohort", { cohortId, onProgress: progressChannel(onProgress) })
      : mock.regenerateCohort(cohortId, onProgress),
  getLatestCohort: (projectId: number): Promise<Cohort | null> =>
    inTauri ? invoke("get_latest_cohort", { projectId }) : mock.getLatestCohort(),
  getCohortSummary: (cohortId: number): Promise<CohortSummary> =>
    inTauri ? invoke("get_cohort_summary", { cohortId }) : mock.getCohortSummary(),
  listRespondents: (cohortId: number, query: string, offset: number, limit: number): Promise<RespondentPage> =>
    inTauri ? invoke("list_respondents", { cohortId, query, offset, limit }) : mock.listRespondents(query, offset, limit),
  getRespondent: (respondentId: number): Promise<RespondentDetail> =>
    inTauri ? invoke("get_respondent", { respondentId }) : mock.getRespondent(respondentId),
  lockCohort: (cohortId: number): Promise<Cohort> => (inTauri ? invoke("lock_cohort", { cohortId }) : mock.lockCohort()),
  hasApiKey: (): Promise<boolean> => (inTauri ? invoke("has_api_key") : Promise.resolve(true)),
  setApiKey: (key: string): Promise<void> => (inTauri ? invoke("set_api_key", { key }) : Promise.resolve()),
  testConnection: (): Promise<string[]> => (inTauri ? invoke("test_connection") : Promise.resolve(["gemini-mock-flash"])),
};
