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
import type { CrossTab } from "../types/gen/CrossTab";
import type { ExportFormat } from "../types/gen/ExportFormat";
import type { Question } from "../types/gen/Question";
import type { Report } from "../types/gen/Report";
import type { QuestionBody } from "../types/gen/QuestionBody";
import type { RunConfig } from "../types/gen/RunConfig";
import type { RunProgress } from "../types/gen/RunProgress";
import type { SimulationRun } from "../types/gen/SimulationRun";
import type { Survey } from "../types/gen/Survey";
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

function progressChannel<T = CohortProgress>(onProgress: (p: T) => void): Channel<T> {
  const ch = new Channel<T>();
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
  getSurvey: (projectId: number): Promise<Survey> => (inTauri ? invoke("get_survey", { projectId }) : mock.getSurvey()),
  redraftSurvey: (projectId: number): Promise<Survey> => (inTauri ? invoke("redraft_survey", { projectId }) : mock.redraftSurvey()),
  updateSurveyText: (surveyId: number, title: string, intro: string): Promise<Survey> =>
    inTauri ? invoke("update_survey_text", { surveyId, title, intro }) : mock.updateSurveyText(title, intro),
  updateQuestion: (questionId: number, body: QuestionBody): Promise<Question> =>
    inTauri ? invoke("update_question", { questionId, body }) : mock.updateQuestion(questionId, body),
  critiqueQuestion: (questionId: number): Promise<Question> =>
    inTauri ? invoke("critique_question", { questionId }) : mock.critiqueQuestion(questionId),
  reorderQuestions: (surveyId: number, orderedIds: number[]): Promise<Survey> =>
    inTauri ? invoke("reorder_questions", { surveyId, orderedIds }) : mock.reorderQuestions(orderedIds),
  addQuestion: (surveyId: number): Promise<Question> => (inTauri ? invoke("add_question", { surveyId }) : mock.addQuestion()),
  deleteQuestion: (questionId: number): Promise<Survey> =>
    inTauri ? invoke("delete_question", { questionId }) : mock.deleteQuestion(questionId),
  approveQuestion: (questionId: number): Promise<Question> =>
    inTauri ? invoke("approve_question", { questionId }) : mock.approveQuestion(questionId),
  addSuggestion: (questionId: number): Promise<Survey> =>
    inTauri ? invoke("add_suggestion", { questionId }) : mock.addSuggestion(questionId),
  suggestMore: (surveyId: number): Promise<Survey> =>
    inTauri ? invoke("suggest_more", { surveyId }) : mock.suggestMore(),
  startSimulation: (projectId: number, config: RunConfig, onProgress: (p: RunProgress) => void): Promise<SimulationRun> =>
    inTauri
      ? invoke("start_simulation", { projectId, config, onProgress: progressChannel<RunProgress>(onProgress) })
      : mock.startSimulation(onProgress),
  getLatestRun: (projectId: number): Promise<SimulationRun | null> =>
    inTauri ? invoke("get_latest_run", { projectId }) : mock.getLatestRun(),
  pauseRun: (runId: number): Promise<void> => (inTauri ? invoke("pause_run", { runId }) : mock.pauseRun()),
  resumeRun: (runId: number, onProgress: (p: RunProgress) => void): Promise<SimulationRun> =>
    inTauri ? invoke("resume_run", { runId, onProgress: progressChannel<RunProgress>(onProgress) }) : mock.resumeRun(onProgress),
  stopRun: (runId: number): Promise<SimulationRun> => (inTauri ? invoke("stop_run", { runId }) : mock.stopRun()),
  getReport: (runId: number): Promise<Report> => (inTauri ? invoke("get_report", { runId }) : mock.getReport()),
  getCrosstab: (runId: number, questionId: number, dimension: string): Promise<CrossTab> =>
    inTauri ? invoke("get_crosstab", { runId, questionId, dimension }) : mock.getCrosstab(questionId, dimension),
  regenerateSynthesis: (runId: number): Promise<Report> =>
    inTauri ? invoke("regenerate_synthesis", { runId }) : mock.regenerateSynthesis(),
  /** Opens the save dialog; resolves to the saved path, or null if cancelled. */
  exportRun: (runId: number, format: ExportFormat): Promise<string | null> =>
    inTauri ? invoke("export_run", { runId, format }) : mock.exportRun(format),
  hasApiKey: (): Promise<boolean> => (inTauri ? invoke("has_api_key") : Promise.resolve(true)),
  setApiKey: (key: string): Promise<void> => (inTauri ? invoke("set_api_key", { key }) : Promise.resolve()),
  testConnection: (): Promise<string[]> => (inTauri ? invoke("test_connection") : Promise.resolve(["gemini-mock-flash"])),
};
