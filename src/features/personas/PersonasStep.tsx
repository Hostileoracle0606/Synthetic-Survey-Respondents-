import { StepPlaceholder } from "../../components/StepPlaceholder";

export function PersonasStep() {
  return (
    <StepPlaceholder
      title="Project"
      heading="Persona Review"
      subtitle="Check who is in the cohort before writing questions."
      milestone="M2"
      items={["Progress while personas are generated", "Five summary cards", "Respondent grid with search", "Profile drawer", "Regenerate Cohort and Proceed to Questionnaire"]}
    />
  );
}
