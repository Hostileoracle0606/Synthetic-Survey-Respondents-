import { beforeEach, describe, expect, it } from "vitest";
import { CONSOLE_LIMIT, useRun } from "./run";
import type { RunProgress } from "../types/gen/RunProgress";

function batch(n: number, from = 0): RunProgress {
  return {
    kind: "batch",
    answered: from + n,
    totalAnswers: 2000,
    respondentsDone: 1,
    costUsd: null,
    avgLatencyMs: 4000,
    p95LatencyMs: 8000,
    answersPerMin: 60,
    concurrency: 4,
    deltas: [
      { questionId: 1, respondentId: 1, code: "B", codes: null, value: null },
      { questionId: 2, respondentId: 1, code: null, codes: ["A", "C"], value: null },
      { questionId: 3, respondentId: 1, code: null, codes: null, value: 5 },
    ],
    console: Array.from({ length: n }, (_, i) => ({ at: "0", respondent: 1, question: `Q${i}`, answer: "x", reason: "r" })),
  };
}

describe("run store", () => {
  beforeEach(() => useRun.getState().reset());

  it("counts answers per option and scale value from deltas", () => {
    useRun.getState().apply(batch(3));
    useRun.getState().apply(batch(3, 3));
    const { counts, stats } = useRun.getState();
    expect(counts[1]).toEqual({ B: 2 });
    expect(counts[2]).toEqual({ A: 2, C: 2 });
    expect(counts[3]).toEqual({ "5": 2 });
    expect(stats?.answered).toBe(6);
  });

  it("keeps only the last 500 console lines, events included", () => {
    for (let i = 0; i < 30; i++) useRun.getState().apply(batch(20, i * 20));
    useRun.getState().apply({ kind: "event", level: "warn", text: "Rate limited" });
    const { log } = useRun.getState();
    expect(log).toHaveLength(CONSOLE_LIMIT);
    expect(log[log.length - 1]).toMatchObject({ kind: "event", text: "Rate limited" });
  });

  it("tracks status messages", () => {
    useRun.getState().apply({ kind: "status", status: "paused" });
    expect(useRun.getState().status).toBe("paused");
  });
});
