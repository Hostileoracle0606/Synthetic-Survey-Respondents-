import { useWizard } from "./store/wizard";
import { SurveyInfoStep } from "./features/survey-info/SurveyInfoStep";
import { PersonasStep } from "./features/personas/PersonasStep";
import { QuestionnaireStep } from "./features/questionnaire/QuestionnaireStep";
import { SimulationStep } from "./features/simulation/SimulationStep";
import { ReportStep } from "./features/report/ReportStep";

const SCREENS = [SurveyInfoStep, PersonasStep, QuestionnaireStep, SimulationStep, ReportStep];

export default function App() {
  const step = useWizard((s) => s.step);
  const Screen = SCREENS[step];
  return <Screen />;
}
