-- B23: distribution mode. A run can ask for per-option probabilities on single-choice
-- questions (docs/SPEC.md §8) when the answering model supports log-probabilities. Off by
-- default; snapshotted on the run like every other model setting.
ALTER TABLE simulation_runs ADD COLUMN logprobs INTEGER NOT NULL DEFAULT 0 CHECK (logprobs IN (0,1));
