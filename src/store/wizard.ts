import { create } from "zustand";
import type { Cohort } from "../types/gen/Cohort";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { CohortProgress } from "../types/gen/CohortProgress";
import type { SurveyInfo } from "../types/gen/SurveyInfo";

export const STEPS = ["Survey Info", "Personas", "Questionnaire", "Simulation", "Report"] as const;
export type StepIndex = 0 | 1 | 2 | 3 | 4;

type WizardState = {
  step: StepIndex;
  /** Furthest step whose precondition holds (docs/DATA_FLOW.md §2); later steps are locked. */
  reached: StepIndex;
  projectId: number | null;
  info: SurveyInfo;
  cohort: CohortConfig;
  /** The cohort being shown on Step 2 and its live generation progress. */
  currentCohort: Cohort | null;
  progress: CohortProgress | null;
  goTo: (s: StepIndex) => void;
  reach: (s: StepIndex) => void;
  setInfo: (patch: Partial<SurveyInfo>) => void;
  setCohort: (patch: Partial<CohortConfig>) => void;
  setProjectId: (id: number) => void;
  setCurrentCohort: (c: Cohort | null) => void;
  setProgress: (p: CohortProgress | null) => void;
};

export const useWizard = create<WizardState>((set) => ({
  step: 0,
  reached: 0,
  projectId: null,
  info: { title: "", researchType: null, productCategory: "mobile_phone", countries: [], researchGoal: "" },
  cohort: { size: 200, seed: 4821, quotas: [], screening: "", nonBinaryShare: 0, countries: [] },
  currentCohort: null,
  progress: null,
  goTo: (s) => set((st) => (s <= st.reached ? { step: s } : st)),
  reach: (s) => set((st) => ({ reached: (Math.max(st.reached, s) as StepIndex), step: s })),
  setInfo: (patch) => set((st) => ({ info: { ...st.info, ...patch } })),
  setCohort: (patch) => set((st) => ({ cohort: { ...st.cohort, ...patch } })),
  setProjectId: (id) => set({ projectId: id }),
  setCurrentCohort: (c) => set({ currentCohort: c }),
  setProgress: (p) => set({ progress: p }),
}));
