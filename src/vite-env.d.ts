/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** "1" only in performance-harness builds (src/lib/perf.ts). */
  readonly VITE_PERF_HARNESS?: string;
}
