import { create } from "zustand";
import type { ConsoleLine } from "../types/gen/ConsoleLine";
import type { EventLevel } from "../types/gen/EventLevel";
import type { RunProgress } from "../types/gen/RunProgress";
import type { RunStatus } from "../types/gen/RunStatus";
import type { SimulationRun } from "../types/gen/SimulationRun";

/** The UI keeps the last 500 console lines; the full history stays in the database. */
export const CONSOLE_LIMIT = 500;

export type LogLine = { key: number; kind: "answer"; line: ConsoleLine } | { key: number; kind: "event"; level: EventLevel; text: string; at: string };

export type Stats = {
  answered: number;
  totalAnswers: number;
  respondentsDone: number;
  costUsd: number | null;
  avgLatencyMs: number;
  p95LatencyMs: number;
  answersPerMin: number;
  concurrency: number;
};

type RunState = {
  run: SimulationRun | null;
  status: RunStatus | null;
  stats: Stats | null;
  log: LogLine[];
  /** questionId → answer key (option code or scale value) → count, from live deltas. */
  counts: Record<number, Record<string, number>>;
  setRun: (r: SimulationRun | null) => void;
  apply: (p: RunProgress) => void;
  reset: () => void;
};

let seq = 0;

export const useRun = create<RunState>((set) => ({
  run: null,
  status: null,
  stats: null,
  log: [],
  counts: {},
  setRun: (run) => set({ run, status: run?.status ?? null }),
  reset: () => set({ run: null, status: null, stats: null, log: [], counts: {} }),
  apply: (p) =>
    set((st) => {
      if (p.kind === "status") return { status: p.status };
      if (p.kind === "event") {
        const line: LogLine = { key: seq++, kind: "event", level: p.level, text: p.text, at: String(Date.now()) };
        return { log: [...st.log, line].slice(-CONSOLE_LIMIT) };
      }
      const counts = { ...st.counts };
      for (const d of p.deltas) {
        const keys = d.codes ?? (d.code != null ? [d.code] : d.value != null ? [String(d.value)] : []);
        const q = { ...(counts[d.questionId] ?? {}) };
        for (const k of keys) q[k] = (q[k] ?? 0) + 1;
        counts[d.questionId] = q;
      }
      const lines: LogLine[] = p.console.map((line) => ({ key: seq++, kind: "answer", line }));
      return {
        stats: {
          answered: p.answered,
          totalAnswers: p.totalAnswers,
          respondentsDone: p.respondentsDone,
          costUsd: p.costUsd,
          avgLatencyMs: p.avgLatencyMs,
          p95LatencyMs: p.p95LatencyMs,
          answersPerMin: p.answersPerMin,
          concurrency: p.concurrency,
        },
        counts,
        log: [...st.log, ...lines].slice(-CONSOLE_LIMIT),
      };
    }),
}));
