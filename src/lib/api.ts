/**
 * Typed wrappers around Tauri commands (src-tauri/src/commands.rs). Types come from
 * src/types/gen, generated from survey-core by `cargo test`.
 *
 * Outside the desktop app (plain `pnpm dev` in a browser) a small in-memory mock is used,
 * so screens can be worked on without the Rust side.
 */
import { invoke } from "@tauri-apps/api/core";
import countries from "../../crates/core/data/countries.json";
import type { AppError } from "../types/gen/AppError";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { CountryOption } from "../types/gen/CountryOption";
import type { Project } from "../types/gen/Project";
import type { SurveyInfo } from "../types/gen/SurveyInfo";

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
  return isAppError(e) ? e.message : String(e);
}

let mockProject: Project | null = null;
const mock = {
  list_countries: async (): Promise<CountryOption[]> => countries as CountryOption[],
  save_survey_info: async (args: { projectId: number | null; info: SurveyInfo }): Promise<Project> => {
    const now = new Date().toISOString();
    mockProject = {
      id: args.projectId ?? 1,
      title: args.info.title,
      researchType: args.info.researchType,
      productCategory: args.info.productCategory,
      countries: args.info.countries,
      researchGoal: args.info.researchGoal,
      wizardStep: 1,
      createdAt: mockProject?.createdAt ?? now,
      updatedAt: now,
    };
    return mockProject;
  },
  generate_cohort: async (): Promise<void> => {
    throw { code: "not_implemented", message: "Persona generation is planned for M2 (browser preview)" } satisfies AppError;
  },
};

function call<T>(cmd: keyof typeof mock, args?: Record<string, unknown>): Promise<T> {
  if (inTauri) return invoke<T>(cmd, args);
  return (mock[cmd] as (a?: Record<string, unknown>) => Promise<T>)(args);
}

export const api = {
  listCountries: () => call<CountryOption[]>("list_countries"),
  saveSurveyInfo: (projectId: number | null, info: SurveyInfo) => call<Project>("save_survey_info", { projectId, info }),
  generateCohort: (projectId: number, config: CohortConfig) => call<void>("generate_cohort", { projectId, config }),
};
