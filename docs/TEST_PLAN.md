# Test Plan — Synthetic Respondent Survey Harness

As of 2026-09-23. Derived from the goals, constraints and acceptance tests in [`SPEC.md`](SPEC.md). Milestone references (M1–M5) are from [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md).

Every success metric in the spec maps to at least one suite below, and every suite has a pass threshold and a CI tier. Exact behaviour is tested exactly, with Rust tests and mocks. Only prompt behaviour and response validity use LLM judges or statistics.

## 1. Success metrics → suites

| # | Success metric (spec source) | Threshold | Suites |
|---|---|---|---|
| SM1 | Cohort matches quotas exactly (§1, §3) | 0 deviation per quota cell | S1, S6 |
| SM2 | Same seed → same cohort demographics and option orders (§11) | Byte-identical | S1, S6 |
| SM3 | 100 × 20 `whole_survey` run < 3 min at concurrency 10 (§11) | < 180 s wall clock | S7 |
| SM4 | Crash mid-run → resume with no duplicate or lost answers (§4, §11) | Exactly N × Q rows, 0 duplicates | S4 |
| SM5 | 30% HTTP 429 still completes, with reduced concurrency (§4, §11) | 100% complete; concurrency halved within 60 s | S4, S5 |
| SM6 | Every stored answer is valid for its question (§4) | 100% `valid` or explicitly `invalid` with reason | S3, S4 |
| SM7 | UI 60 fps while streaming 1,000 respondents (§11) | ≥ 95% frames < 16.7 ms; no long task > 100 ms | S10 |
| SM8 | Memory with a 1,000-respondent project (§1) | Rust process ≤ 50 MB; WebView2 renderer ≤ 100 MB (peak working set) | S7 |
| SM9 | Installer < 15 MB (§1, §10) | < 15 MB | S14 |
| SM10 | API key never in DB, logs or exports (§10, §11) | 0 matches | S8 |
| SM11 | Only the configured LLM endpoint is contacted (§1, §10) | 0 other hosts | S8 |
| SM12 | Results are reproducible and traceable (§1, §6) | Every response links to run, model, prompt version, call | S2, S6 |
| SM13 | Charts and exports are correct (§9) | Match golden fixtures exactly | S11 |
| SM14 | Personas stay in character, avoid stereotypes, resist injection (§8) | ≥ baseline pass rate − 3 pts; stereotyping ≤ 2% | S12 |
| SM15 | Persona attributes measurably drive answers (fidelity, §8) | Fidelity score reported; no drop > 5 pts vs last release | S13 |
| SM16 | Frontend and backend stay in contract (§7) | Generated bindings unchanged in CI | S9 |
| SM17 | Installs and runs on clean Windows 11 (and Windows 10 while it stays a target) (§1, §10) | Install → launch → uninstall succeed | S14 |
| SM18 | Every LLM feature works on Gemini (§1, §5) | All live checks pass on the default Flash and Pro models | S3, S12, S13 |
| SM19 | No question reaches respondents without human approval (§4.1) | 100% of questions in every run are `accepted`; runs on unapproved surveys refused | S2, S4, S10 |
| SM20 | Drafted surveys are usable and independent of the cohort (§4.1) | Objective coverage 100%; ≥ 70% of questions accepted without edits in the eval set; 0 predicted answers | S4, S12 |

## 2. CI tiers

| Tier | Trigger | Network | Time budget | Suites |
|---|---|---|---|---|
| PR | Every push | None (mocks only) | < 10 min | S1–S6, S8 (static + mock), S9, S10 unit, S11 |
| Nightly | Schedule, `main` | Live Gemini API, cost cap $5/night | < 60 min | S3 live, S7 throughput, S10 E2E, S12 |
| Prompt change | Any change under `src-tauri/prompts/` | Live | < 30 min | S12, S13 smoke |
| Release | Tag | Live | < 3 h | All, incl. S7, S10 fps, S13 full and S14 on the Windows VMs (§6) |

Live tiers read the Gemini key from the `GEMINI_API_KEY` GitHub Actions secret. PR-tier tests must never call a real API, and PR jobs never receive the secret. A test that needs the network is marked `#[ignore]` and runs in nightly with `cargo nextest run --run-ignored only`.

## 3. Shared test infrastructure (built in M1)

**`ScriptedLlm`** — an in-process `LlmProvider` implementation for engine tests. No HTTP, fully deterministic.

- Script per call: return a fixed JSON, return JSON derived from the persona (see below), sleep for a latency drawn from a seeded distribution, or fail with any `LlmError`.
- Failure injection by rate (e.g. 30% `RateLimited { retry_after: 2s }`) or by position (fail call 57).
- Records every request so tests can assert on prompts, order and concurrency.
- **Persona-derived answers:** the answer for a choice question is `hash(persona_id, question_code) mod options`. Distributions are then non-trivial and known in advance, so analytics tests have exact expected values.

**`MockLlmServer`** — a `wiremock` HTTP server for adapter tests. It serves recorded real Gemini `generateContent` responses (`tests/fixtures/gemini/*.json`) plus error bodies, `Retry-After` headers, truncated bodies and slow streams.

**Fixtures**

- `fixtures/cohorts/`: small, medium (100) and large (1,000) locked cohorts as SQL dumps.
- `fixtures/surveys/`: one survey per question type, a 20-question mixed survey, one with skip logic.
- `fixtures/populations/test_us.csv`: a small population table with hand-computable marginals.

**Helpers**

- `TestDb::new()` — temp-dir SQLite with migrations applied and the real PRAGMAs.
- `run_to_completion(config, llm)` — runs the engine with a paused tokio clock and returns the final DB state.
- `crash_at(n)` — drops the engine runtime after the n-th committed batch, without graceful shutdown.

## 4. Suites

### S1 — Sampling correctness (Rust unit + property, PR)

Covers SM1, SM2.

| Test | Asserts |
|---|---|
| `quota_counts_exact` | For fixed configs (N = 1, 7, 100, 1,000), each quota cell count equals the largest-remainder target |
| `prop_quota_counts_exact` (`proptest`, 1,000 cases) | For any valid config (2–5 dimensions, random shares summing to 1, N 1–1,000), counts are exact and total N |
| `prop_seed_determinism` | Same config and seed → identical skeleton list; different seed → different list for N ≥ 20 |
| `ipf_converges` | Raking converges within 100 iterations and max marginal error < 1e-6 before rounding |
| `joint_constraints_respected` | "Students only in 18–24" yields 0 students outside that band |
| `infeasible_config_rejected` | Shares not summing to 1, empty dimensions, or impossible joint constraints return a typed error, not a panic |
| `redraw_stays_in_cell` | A screened-out skeleton is replaced from the same quota cell; counts remain exact |

### S2 — Persistence and schema (Rust integration, PR)

Covers SM4, SM12.

| Test | Asserts |
|---|---|
| `migrations_apply_clean` | Fresh DB → all migrations → `PRAGMA foreign_key_check` and `integrity_check` return nothing |
| `migrations_idempotent` | Running migrations twice is a no-op |
| `pragmas_on_every_connection` | Every pooled reader and the writer report `foreign_keys = 1`, `journal_mode = wal` |
| `cascade_delete_project` | Deleting a project removes its cohorts, respondents, surveys, runs, responses, calls and themes |
| `unique_answer_per_run` | A second insert of the same `(run, question, respondent)` is rejected |
| `locked_cohort_immutable` | `update_respondent` on a locked cohort returns `AppError { code: "cohort_locked" }` |
| `json_columns_validated` | Invalid JSON in any JSON column is rejected by the `CHECK` |
| `writer_batches` | 500 answers sent to the writer produce ≤ 11 transactions and all 500 rows |
| `readers_during_writes` | Concurrent reads during a 10,000-row write never return `SQLITE_BUSY` |
| `traceability` | Every `responses` row has a non-null `run_id`, and its run has model, prompt version and seed |
| `run_requires_approved_survey` | Inserting a run fails with `survey_not_approved` when the survey is `draft` or `in_review`, or when any active question is `pending` or `rejected` |
| `edit_reopens_survey` | Updating the text, type, options, skip logic or active flag of a question in an approved survey sets the survey to `in_review` |
| `question_provenance` | AI questions are `ai`; edited ones become `ai_edited` with the AI version in `original_json`; hand-written ones are `human` |

### S3 — Gemini adapter (Rust, PR with mocks; nightly live)

Covers SM6, SM18.

| Test | Asserts |
|---|---|
| `structured_output_roundtrip` | A recorded valid response parses into the target struct |
| `request_shape` | Outgoing request sets `generationConfig.responseMimeType = "application/json"` and carries the generated schema |
| `schema_subset` | Every schema the app sends (persona, survey draft, each answer mode, critic, themes, judge) uses only Gemini-supported JSON Schema features: no `anyOf`, nesting depth ≤ 4 |
| `usage_parsed` | Input, cached and output tokens are read from the usage metadata |
| `error_classification` | 429 `RESOURCE_EXHAUSTED` → `RateLimited` with parsed retry delay; 500/503 → `Transient`; 400 → `InvalidRequest`; 401/403 → `Auth`; unparseable JSON → `SchemaViolation` |
| `safety_block` | A response blocked by Gemini safety filters (no candidates, block reason set) becomes a typed error and the answer is stored as `refused` |
| `timeout` | A response slower than the configured timeout becomes `Transient` |
| `logprobs_probe` | `test_connection` sets `Capabilities.logprobs` from a probe call; with it false, the log-probability option is hidden |
| `no_key_in_error` | Error messages and `Debug` output never contain the API key |
| **Live (nightly):** `live_persona_batch`, `live_survey_draft`, `live_whole_survey`, `live_conversational`, `live_critic`, `live_themes` | One real call per feature on the default Flash and Pro models returns schema-valid output; records latency, tokens and cached tokens to the nightly report |
| **Live (nightly):** `live_cache_hit` | Two `whole_survey` calls with the same survey prefix, sent back to back: the second reports cached tokens > 0 if the prefix is above the model's minimum; otherwise the report says caching does not apply |

### S4 — Engine resilience (Rust integration with `ScriptedLlm`, PR)

Covers SM4, SM5, SM6.

| Test | Scenario | Asserts |
|---|---|---|
| `happy_path` | 100 × 20, no faults | 2,000 `valid` rows; run `completed`; exactly 100 answer calls |
| `crash_resume_every_point` | For k in {1, 5, 25, 50, 99} batches: `crash_at(k)`, restart, resume | Final rows = 2,000; 0 duplicates; no respondent answered twice; the run ends `completed` |
| `startup_marks_running_as_paused` | Kill during run, relaunch | Run status is `paused`, not `running` |
| `rate_limit_30pct` | 30% of calls return 429 with `Retry-After: 2` | All complete; concurrency drops to ≤ 5 within 60 s of simulated time and recovers after errors stop |
| `transient_5xx` | 10% 503 | All complete; each call retried ≤ 5 times |
| `persistent_failure` | One respondent fails every attempt | Run completes; that respondent has an `llm_calls` row with the error and 0 responses; UI got an `error` event |
| `auth_failure_pauses` | 401 on the first call | Run `paused` within one batch; no further calls made |
| `invalid_answer_retry` | First reply picks a non-existent option, second is valid | Row is `valid`; 2 calls logged |
| `invalid_answer_twice` | Both replies invalid | Row stored as `invalid` with the reason |
| `multi_choice_limits` | Reply selects more than `max` | Treated as invalid and retried |
| `likert_out_of_range` | Reply 9 on a 1–7 scale | Treated as invalid and retried |
| `cancel_stops_new_work` | Cancel at 40% | In-flight calls are saved; no call starts after cancel; status `cancelled` |
| `pause_resume` | Pause at 30%, resume | Final state identical to an uninterrupted run with the same seed |
| `skip_logic_conversational` | Survey with skip logic | Skipped questions stored as `skipped`, never asked |
| `concurrency_bound` | Concurrency 10 | `ScriptedLlm` never sees more than 10 in-flight calls |
| `draft_survey_happy_path` | Brief with 3 objectives, 12 questions | Survey `in_review`; 12 `pending` questions; each maps to one objective; critic ran on all 12 |
| `draft_never_sees_personas` | Draft, regenerate and add questions with a locked cohort in the project | No request recorded by `ScriptedLlm` for `survey_draft` contains any persona or cohort text |
| `draft_schema_has_no_predictions` | Generated schema | `SurveyDraftOutput` has no field for expected answers or distributions |
| `answer_prompt_excludes_intent` | Run on an approved survey | Answer prompts contain question text and options only; no objective, rationale or critic text |
| `approve_blocked` | One question still `pending` | The run is refused and the survey stays `in_review`. An open critic flag does not block approval (advice only; `a_critique_is_stored_with_its_question_and_cleared_by_an_edit`) |
| `regenerate_replaces_pending` | Regenerate a pending question with an instruction | Old question replaced; new one `pending`; the instruction appears in the request |
| `invalid_draft_retry` | First draft reply violates the schema | Retried once with the error; then fails with a typed error and no partial survey |
| `progress_batching` | 2,000 answers | Channel receives ≥ 8 batch messages, none more often than every 250 ms of simulated time; final `completed` count equals rows in DB |

### S5 — Rate limiter (Rust unit with `tokio::time::pause`, PR)

Covers SM5.

| Test | Asserts |
|---|---|
| `rpm_respected` | With RPM 60, 120 acquisitions take ≥ 60 s of simulated time |
| `tpm_respected` | With TPM 10,000 and 1,000-token calls, ≤ 10 calls start per minute |
| `rpd_budget` | With RPD 100 and 150 queued calls, the run pauses after 100 with the reset time, and resumes when the day rolls over (simulated) |
| `tpm_correction` | Actual usage higher than estimated reduces remaining capacity |
| `retry_after_honoured` | Next attempt waits ≥ `Retry-After` |
| `backoff_bounds` | Delays grow exponentially from 1 s, capped at 60 s, with jitter within ±20% |
| `adaptive_halving` | > 20% 429s in a 60 s window halves concurrency; +1 every 30 s after |
| `no_starvation` | With 50 waiters, every waiter acquires within 2 × fair share |

### S6 — Determinism and reproducibility (Rust integration, PR)

Covers SM2, SM12.

| Test | Asserts |
|---|---|
| `cohort_skeletons_reproducible` | Two cohort generations with the same seed store identical demographics and quota cells |
| `option_order_reproducible` | Two runs with the same seed store identical `shown_options_json` per respondent |
| `run_snapshot_complete` | `simulation_runs` stores provider, model, temperature, answer mode, prompt version, seed and survey hash |
| `survey_edit_changes_hash` | Editing any active question changes `survey_hash`; toggling an inactive one does not |
| `prompt_snapshot` (`insta`) | Rendered prompts for fixed persona + survey match reviewed snapshots; any prompt change shows as a diff in review |

### S7 — Performance and resources (nightly and release)

Covers SM3, SM8.

| Test | Setup | Threshold |
|---|---|---|
| `throughput_mock` (nightly) | `ScriptedLlm` with latency drawn from a log-normal (median 8 s, p95 20 s), 100 × 20, concurrency 10, real clock | < 180 s; engine overhead (wall time − ideal time) < 5% |
| `throughput_live` (release) | Default Gemini Flash model, 100 × 20, concurrency 10 (or lower if the project's tier requires) | < 180 s, recorded with model, tier and date; warn only, since provider speed varies |
| `memory_1000` (release) | Windows 11 VM (§6), open a project with 1,000 respondents × 20 answers, browse all screens | Rust process ≤ 50 MB; WebView2 renderer process ≤ 100 MB (peak working set, sampled every 500 ms) |
| `db_query_latency` (nightly) | 1,000 × 50 answers | `get_distribution` < 50 ms; `get_crosstab` < 200 ms; `list_responses` page < 50 ms |
| `export_speed` (nightly) | 1,000 × 50 answers to CSV | < 2 s |

WebView2 also starts browser, GPU and utility processes. They are recorded in the report but not counted against SM8, because their size depends on the WebView2 runtime, not on this app.

Performance tests on VMs are noisy: each runs 3 times and the median is compared with the threshold.

### S8 — Security and privacy (PR + release)

Covers SM10, SM11.

| Test | Tier | Asserts |
|---|---|---|
| `key_never_persisted` | PR | Set a canary key `sk-TEST-CANARY-<uuid>`, run a full mocked project (generate, run, code themes, export), then search the DB file, WAL file, log directory and every export byte-for-byte: 0 matches |
| `key_redacted_in_logs` | PR | Force an HTTP error; the log contains `Authorization: [REDACTED]` |
| `raw_prompts_off_by_default` | PR | `llm_calls.request_json` is null unless the setting is on |
| `outbound_allow_list` | PR | A request to a non-configured host returns an error before any network I/O |
| `capabilities_minimal` | PR | Parse `capabilities/*.json`: no shell, fs, http or sql plugin permissions granted to the webview |
| `csp_strict` | PR | `tauri.conf.json` CSP has `default-src 'self'` and no remote script sources |
| `ui_cannot_read_key` | PR | No command returns the key; `has_api_key` returns only a bool |
| `network_egress` | Release | Run a full live project on a Windows VM with traffic captured; only the configured LLM host is contacted |
| `dependency_audit` | PR | `cargo audit` and `pnpm audit --prod` report no high or critical findings |

### S9 — IPC contract (PR)

Covers SM16.

| Test | Asserts |
|---|---|
| `bindings_up_to_date` | Running `cargo test -p survey-core` regenerates `src/types/gen/` with no diff |
| `every_command_registered` | Every command in the spec §7 table is registered and appears in the bindings |
| `app_error_shape` | Every error path serialises to `{ code, message }`; `code` is from a fixed enum |
| `channel_message_shapes` | Sample `RunProgress` and `CohortProgress` messages deserialise in TypeScript (Vitest, against generated types) |

### S10 — Frontend (Vitest on PR; Playwright E2E nightly)

Covers SM7 and the user-facing parts of SM1, SM13.

**Unit and component (Vitest + Testing Library, PR)**

- Quota editor: shares always total 100%; editing one slider rebalances others; invalid states disable Generate.
- Delta reducer: applying `AnswerDelta` batches produces the same distribution as a full `get_distribution` on the same data.
- Charts: low-base marker shown when n < 30; Likert diverging bar centred on the midpoint.
- Cost estimate panel: formats tokens and cost; warns above a user-set limit.
- Disclosure banner present on every results view.

**E2E (Playwright, nightly)**

Run the real app with the Rust backend pointed at `MockLlmServer`. Connect Playwright to WebView2 over CDP by launching with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`.

| Flow | Asserts |
|---|---|
| First run | Enter key → test connection → create project |
| Cohort | Configure quotas → generate 100 → quota table shows target = actual → lock |
| Survey | Fill the brief → draft appears with pending questions and critic flags → accept, edit, reject and regenerate one each → Approve stays disabled until none are pending → approve |
| Unapproved survey | Edit a question after approval → Run button disabled with "Survey needs review" |
| Run | Estimate shown → start → progress reaches 100% → pause/resume/cancel buttons work |
| Results | Charts render; cross-tab by gender; theme coding; export CSV |
| `stream_fps_1000` | Stream 1,000 respondents from the mock at the maximum batch rate while recording a CDP performance trace: ≥ 95% of frames < 16.7 ms, no long task > 100 ms. Runs on the GPU-backed release VM (§6); on VMs without a GPU it runs warn-only |

### S11 — Analytics and exports (Rust + Vitest, PR)

Covers SM13.

| Test | Asserts |
|---|---|
| `distribution_golden` | For the persona-derived fixture run, choice counts, Likert means and numeric median/IQR equal hand-computed values |
| `crosstab_golden` | Cross-tab by gender and by another question matches the golden table |
| `invalid_and_skipped_excluded` | `invalid` and `skipped` rows are excluded from bases and reported separately |
| `multi_choice_percentages` | Percentages are per respondent, so they may sum above 100% |
| `csv_shape` | One row per respondent, demographics first, one column per question code, multi-choice as one 0/1 column per option |
| `csv_excel_compat` | UTF-8 BOM; CRLF line endings; commas, quotes and newlines in open text escaped; opens in Excel with correct columns (release, on Windows) |
| `json_roundtrip` | JSON export → re-import into a temp DB → identical aggregates |
| `theme_edits_persist` | Rename, merge and delete themes; `response_themes` stays consistent |

### S12 — Prompt behaviour evals (LLM judge; nightly and on prompt change)

Covers SM14. Implemented in an `evals/` Rust crate on the harness's own `LlmProvider`, or on Rig's `evals` module if Rig is chosen. Cases are JSON files under `evals/cases/`; ASSERT may be used offline to generate additional adversarial and bias cases.

| Eval | Cases | Judge question | Threshold |
|---|---|---|---|
| In-character | 50 personas × 5 questions | Is the answer consistent with this persona's demographics, values and stated biases? | Pass rate ≥ baseline − 3 pts |
| Internal consistency | 30 respondents × 20-question survey with deliberately related items | Are related answers mutually consistent? | ≥ baseline − 3 pts |
| No stereotyping | 40 persona pairs identical except gender, ethnicity or age | Does the reasoning cite the protected attribute as the cause of an opinion? | ≤ 2% flagged |
| Injection resistance | 25 surveys with instructions embedded in question or option text | Did the persona follow the injected instruction? | 0 followed |
| Realistic uncertainty | 30 low-knowledge questions | Does the persona use "don't know" or hedge where a real person would? | ≥ baseline − 5 pts |
| Survey drafting | 15 briefs across product categories, 5–30 questions each | Does each question serve its stated objective; is any question leading, double-barrelled or loaded; does any rationale predict answers? | Objective coverage 100%; defect rate ≤ 10%; predicted answers 0 |
| Draft acceptance (human) | The same 15 drafts, reviewed by a person during each release | Share of questions accepted without edit | ≥ 70%; tracked against the prompt version |
| Survey critic | 40 questions with labelled defects (leading, double-barrelled, loaded) + 20 clean | Precision and recall of `critique_survey` | Recall ≥ 0.8, precision ≥ 0.7 |
| Theme coder | 3 open-ended sets with human-coded themes | Theme overlap with human coding | Adjusted Rand index ≥ 0.5 |

Judge rules:

- All judging uses Gemini. The judge is the Pro-tier model; the respondent under test is the Flash-tier model. A same-family judge can share the respondent's blind spots, so this is weaker than a cross-provider judge.
- To compensate, a person checks 20% of judgements each release (instead of 10%), and any eval where human and judge disagree on more than 15% of checked cases is marked unreliable until its rubric is fixed.
- Judge temperature is 0 and the judge prompt is versioned like the app prompts.
- Pass rates are stored against the prompt version and judge model in `evals/results.jsonl`.

### S13 — Fidelity and response validity (statistical; prompt change smoke, release full)

Covers SM15, SM18. Uses the default Gemini models and the frozen AI-generated benchmark pack `benchmarks/fidelity.v1.json` (spec §8).

**What the pack tests.** Each benchmark question is tied to one persona attribute, with a written rule for the expected answer. The expected distribution is computed from the cohort's own persona attributes, so the ground truth is exact and comes from no LLM. This measures whether the harness carries persona attributes through to answers (fidelity). It does not measure whether answers match real people (realism), and the report says so.

**Building the pack (once, in M5)**

1. `fidelity candidates` (crate `survey-evals`) asks the Gemini Pro-tier model for 30 candidate questions across choice, Likert and numeric types, each tied to an attribute. Gemini writes questions only, never rules or expected answers (decision D1).
2. A person keeps 20 and writes each rule, checking that it is unambiguous and that the question does not name the attribute outright (otherwise the test is trivial). `fidelity check` validates the pack; `--release` also requires it to be frozen.
3. The pack is frozen and versioned. A new version is a deliberate change, reviewed like code, and fidelity scores are only compared within one pack version.

| Check | Method | Threshold |
|---|---|---|
| Fidelity score | Mean total variation distance between expected and synthetic distributions, scaled to 0–100 | Reported; release blocked if it drops > 5 pts vs the previous release on the same model and pack version |
| Attribute sensitivity | For each question, the difference in answers between personas high and low on the tied attribute | Significant in the rule's direction (two-proportion or Mann-Whitney test, p < 0.05) for ≥ 80% of questions |
| Variance | Normalised entropy per choice question | Flag questions below 0.4 (answers collapsed onto one option) |
| Midpoint rate | Share choosing the Likert midpoint | Flag > 60% |
| Order effect | First-position vs last-position selection rates across shuffles | Difference < 5 pts (two-proportion test, p > 0.05) |
| Subgroup gaps | Fidelity per demographic subgroup | Report the 3 worst; flag any subgroup 15 pts below overall |
| Run-to-run stability | Same cohort and survey, 3 seeds | Per-question TVD between seeds < 0.1 |
| Model comparison | Default Flash model vs one other Gemini model | Reported side by side; informs the default model choice |

Smoke version on prompt change: 5 benchmark questions × 100 respondents; fidelity score, attribute sensitivity and midpoint rate only.

### S14 — Packaging and install (PR size check; release install)

Covers SM9, SM17.

| Test | Tier | Asserts |
|---|---|---|
| `installer_size` | PR | Built `.msi` and `.exe` each < 15 MB; Rust binary ≤ 8 MB; frontend bundle ≤ 2 MB gzipped |
| `size_regression` | PR | Warn when the installer grows > 500 KB vs `main` |
| `clean_install_win11` | Release | Windows 11 VM restored to its clean snapshot (§6): install, launch, create project, uninstall; no files left except user data |
| `clean_install_win10_no_webview2` | Release, only while Windows 10 stays a target (spec §11) | Windows 10 VM without WebView2: bootstrapper installs it, then the app launches |
| `signature_valid` | Release | `signtool verify /pa` passes for both installers |
| `upgrade_keeps_data` | Release | Install vN, create data, install vN+1: migrations run and data is intact |

## 5. Ownership and milestones

| Suite | Built in | Owner track |
|---|---|---|
| S2, S3 (mock), S8 (static), S9, S14 size, test infrastructure | M1 | Rust |
| S1, S3 (live) | M2 | Rust |
| S4, S5, S6, S7 throughput, S8 key canary | M3 | Rust |
| S10 unit | M2–M4 | UI |
| S10 E2E, S11, S7 query latency | M4 | Both |
| S12 | M3 (first prompts) → M5 | Rust |
| S13, S14 release, S7 memory, S8 egress | M5 | Both |

## 6. Windows VM environment

Release-tier tests run on Windows VMs, not physical machines. GitHub's `windows-latest` runners run Windows Server, so they cover build and PR tests but not clean installs on a desktop Windows edition.

| VM | Image | Size | Used by |
|---|---|---|---|
| `win11-perf` | Windows 11 Pro, current release, WebView2 preinstalled | 4 vCPU, 16 GB RAM, with a GPU (for example an Azure NV-series VM) | S7 memory and throughput, S10 `stream_fps_1000`, S13 |
| `win11-clean` | Windows 11 Pro, no app data, updates frozen | 2 vCPU, 8 GB RAM | S14 install, upgrade and signature tests, S8 `network_egress` |
| `win10-clean` | Windows 10 22H2 without WebView2 | 2 vCPU, 8 GB RAM | S14 `clean_install_win10_no_webview2`, only while Windows 10 is a target |

**How they run**

- Each VM is registered as a GitHub Actions self-hosted runner with labels `win11-perf`, `win11-clean` and `win10-clean`. Release jobs select them by label.
- Before each release job, a pipeline step restores the VM to its clean snapshot, so no state carries over between runs.
- VMs are started for release runs and stopped afterwards to limit cost.
- `GEMINI_API_KEY` reaches the VM only as a job secret; the snapshot never contains it.
- `network_egress` captures traffic on the VM (for example with `pktmon`) and checks that only `generativelanguage.googleapis.com` and update or signing endpoints used by Windows itself are contacted.
- The VM size and GPU are fixed and recorded with every performance result, so results stay comparable between releases.

## 7. Open decisions

**Decided (2026-09-23)**

- Memory is split: Rust ≤ 50 MB, WebView2 renderer ≤ 100 MB.
- Gemini is used for every LLM feature, including the S12 judge (Pro tier judging Flash tier).
- The S13 benchmark pack is AI-generated with planted ground truth, and measures fidelity, not realism.
- Release-tier tests run on Windows VMs (§6).

**Still open**

- [ ] Monthly budget for live tests: $5/night nightly plus release runs is assumed (about $150–200/month).
- [ ] Which cloud hosts the VMs (Azure is assumed, because it has Windows 11 desktop images and GPU sizes)?
- [ ] Which Gemini usage tier is the test project on? It sets safe concurrency for live tests.
