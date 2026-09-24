import { useWizard, type StepIndex } from "../store/wizard";

/**
 * Performance-harness builds only (`VITE_PERF_HARNESS=1`; TEST_PLAN S7 `memory_1000`, S10
 * `stream_fps_1000`): the harness drives the app over CDP and uses these to open the seeded
 * project and move between steps. Normal builds never load this module.
 */
export function installPerfHooks() {
  Object.assign(window, {
    __perf: {
      open(projectId: number, title: string, step: StepIndex) {
        useWizard.setState((s) => ({ projectId, info: { ...s.info, title }, reached: step, step }));
      },
      goTo: (step: StepIndex) => useWizard.getState().goTo(step),
    },
  });
}
