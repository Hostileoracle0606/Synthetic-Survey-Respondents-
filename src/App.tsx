import { useEffect, useState } from "react";
import { api } from "./lib/api";
import { useWizard } from "./store/wizard";
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
  const Screen = SCREENS[step];
  return (
    <>
      <Screen />
      {needsKey && <KeyPrompt onDone={() => setNeedsKey(false)} />}
    </>
  );
}
