import { STEPS, useWizard, type StepIndex } from "../store/wizard";

export function Stepper() {
  const { step, reached, goTo } = useWizard();
  return (
    <ol aria-label="Survey steps" className="flex items-center">
      {STEPS.map((label, i) => {
        const idx = i as StepIndex;
        const state = idx === step ? "on" : idx < reached || idx < step ? "done" : idx <= reached ? "open" : "locked";
        const cls = {
          on: "bg-olive border-olive text-white",
          done: "border-olive text-olive",
          open: "border-line text-muted hover:border-olive hover:text-olive",
          locked: "border-[#B9B9B3] text-[#8a8a84] cursor-not-allowed",
        }[state];
        return (
          <li key={label} className="flex items-center">
            {i > 0 && <span aria-hidden className="h-px w-4 bg-[#B9B9B3]" />}
            <button
              type="button"
              disabled={state === "locked"}
              aria-current={idx === step ? "step" : undefined}
              onClick={() => goTo(idx)}
              className={`h-12 rounded-full border-[1.5px] px-5 font-display text-[15px] font-medium ${cls}`}
            >
              {i + 1}. {label}
            </button>
          </li>
        );
      })}
    </ol>
  );
}
