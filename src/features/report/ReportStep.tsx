import { StepPlaceholder } from "../../components/StepPlaceholder";

export function ReportStep() {
  return (
    <StepPlaceholder
      title="Project"
      heading="Final Report"
      subtitle="Charts, AI synthesis and offline export."
      milestone="M4"
      items={["Bar, pie and distribution charts by question type", "Cross-tabs with low-base marking", "AI synthesis with checked numbers", "Export Raw CSV and Raw JSON"]}
    />
  );
}
