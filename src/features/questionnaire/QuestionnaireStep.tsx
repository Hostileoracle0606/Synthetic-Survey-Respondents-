import { StepPlaceholder } from "../../components/StepPlaceholder";

export function QuestionnaireStep() {
  return (
    <StepPlaceholder
      title="Project"
      heading="Questionnaire"
      subtitle="Every question needs your approval before the simulation can run."
      milestone="M3"
      items={["Question list with reorder", "Editor for type, prompt and options", "AI suggestions sidebar", "Run Survey Simulation, locked until every question is approved"]}
    />
  );
}
