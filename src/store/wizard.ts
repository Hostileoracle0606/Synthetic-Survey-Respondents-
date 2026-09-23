import { create } from "zustand";
import type { CohortConfig } from "../types/gen/CohortConfig";
import type { SurveyInfo } from "../types/gen/SurveyInfo";

export const STEPS = ["Survey Info", "Personas", "Questionnaire", "Simulation", "Report"] as const;
export type StepIndex = 0 | 1 | 2 | 3 | 4;

type WizardState = {
  step: StepIndex;
  /** Furthest step reached; later steps are not clickable yet. */
  reached: StepIndex;
  projectId: number | null;
  info: SurveyInfo;
  cohort: CohortConfig;
  goTo: (s: StepIndex) => void;
  setInfo: (patch: Partial<SurveyInfo>) => void;
  setCohort: (patch: Partial<CohortConfig>) => void;
  setProjectId: (id: number) => void;
};

export const useWizard = create<WizardState>((set) => ({
  step: 0,
  reached: 0,
  projectId: null,
  info: { title: "", researchType: null, productCategory: "mobile_phone", countries: [], researchGoal: "" },
  cohort: { size: 200, seed: 4821, quotas: [], screening: "" },
  goTo: (s) => set((st) => (s <= st.reached ? { step: s } : st)),
  setInfo: (patch) => set((st) => ({ info: { ...st.info, ...patch } })),
  setCohort: (patch) => set((st) => ({ cohort: { ...st.cohort, ...patch } })),
  setProjectId: (id) => set({ projectId: id }),
}));
