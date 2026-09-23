import { StepPlaceholder } from "../../components/StepPlaceholder";

export function SimulationStep() {
  return (
    <StepPlaceholder
      title="Project"
      heading="Live Simulation"
      subtitle="The Agent answers as each persona; answers are saved as they arrive."
      milestone="M3"
      items={["Answers collected, cost, latency and throughput", "Live console", "Live charts", "Pause Simulation and Stop & Save Progress"]}
    />
  );
}
