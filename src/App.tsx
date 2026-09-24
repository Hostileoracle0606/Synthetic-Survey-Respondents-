import { useEffect, useState } from "react";
import { api } from "./lib/api";
import { useWizard } from "./store/wizard";
import type { StepIndex } from "./store/wizard";
import { KeyPrompt } from "./components/KeyPrompt";
import { SurveyInfoStep } from "./features/survey-info/SurveyInfoStep";
import { PersonasStep } from "./features/personas/PersonasStep";
import { QuestionnaireStep } from "./features/questionnaire/QuestionnaireStep";
import { SimulationStep } from "./features/simulation/SimulationStep";
import { ReportStep } from "./features/report/ReportStep";

const SCREENS = [SurveyInfoStep, PersonasStep, QuestionnaireStep, SimulationStep, ReportStep];

export default function App() {
  const step = useWizard((s) => s.step);
  const [needsKey, setNeedsKey] = useState(false);
  useEffect(() => {
    api.hasApiKey().then((has) => setNeedsKey(!has)).catch(() => setNeedsKey(true));
  }, []);
  useEffect(() => {
    // Relaunching the app reopens the last project on its furthest reached step.
    api.getLastProject().then(async (p) => {
      if (!p) return;
      const { setInfo, setProjectId, reach, setCurrentCohort } = useWizard.getState();
      setInfo({
        title: p.title,
        researchType: p.researchType,
        productCategory: p.productCategory,
        countries: p.countries,
        researchGoal: p.researchGoal,
      });
      setProjectId(p.id);
      const target = Math.min(4, Math.max(0, p.wizardStep - 1)) as StepIndex;
      reach(target);
      if (target >= 1) {
        const cohort = await api.getLatestCohort(p.id).catch(() => null);
        if (cohort) setCurrentCohort(cohort);
      }
    }).catch(() => {});
  }, []);
  const Screen = SCREENS[step];
  return (
    <>
      <Screen />
      {needsKey && <KeyPrompt onDone={() => setNeedsKey(false)} />}
    </>
  );
}
