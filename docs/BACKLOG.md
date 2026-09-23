# Backlog — deferred and unapplied work

As of 2026-09-23. The census population is Canada's (Statistics Canada 2021 Census PUMF), corrected from an earlier US table. Work that was deliberately postponed so M2 could start, plus design changes that were agreed or proposed in discussion but not yet written into the docs. Each item says where it came from and what "done" means. Move an item into a milestone in [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) when it is scheduled.

## Deferred from M1 (not needed by M2)

| # | Item | Why it can wait | Done when |
|---|---|---|---|
| B1 | Full Settings screen: test and delete the key from the UI, model IDs (Flash / Pro), Gemini usage tier, price table | M2 only needs the key, handled by a minimal first-launch prompt. Models are picked automatically (newest stable Flash); rate limits use conservative defaults; prices are first needed in M3 for cost estimates | Settings screen saves all four; the rate limiter and cost estimate read them; first launch opens it when no key is stored |
| B2 | Step 1 autosave on blur, and reopening a project at `projects.wizard_step` | Generate Cohort already saves the project before starting the job | Editing a field saves within 1 s; relaunching the app reopens the last project on its furthest step |
| B3 | Frontend lint (ESLint with React hooks rules) in the web CI job | TypeScript strict mode and tests already run | `pnpm lint` passes and runs in CI |

## Deferred from M2

| # | Item | Why it can wait | Done when |
|---|---|---|---|
| B4 | Mark the cohort "out of date" when Step 1 audience fields change after generation (DATA_FLOW §2) | Users can regenerate by hand; no data is lost | Step 2 shows the banner when the saved `CohortConfig` differs from Step 1 |
| B5 | Census tables for countries other than Canada (the US included) | v1 decision: other countries use quotas-only sampling, flagged in the country list | A table exists per supported country, built by `tools/build-populations` |
| ~~B6~~ | ~~Finer age draw for the 60+ band~~ | **Done in M2:** Canadian ages are drawn from the census age distribution (`CA_ages.csv`; the PUMF gives 5-year groups, so ages are spread evenly within each group); countries without a census table still draw 60+ from 60–84 | — |
| B7 | Non-binary respondents | The census records sex as two categories, so skeleton gender is binary | A documented, user-set share that the sampler applies on top of the census table |
| B8 | Full stepper precondition rules (DATA_FLOW §2), e.g. Step 4 opens only with an approved survey | Steps 3–5 are placeholders until M3/M4; the stepper already blocks steps not yet reached | Each step's precondition is checked in the stepper and by its commands |

## Design changes agreed or proposed but not yet in the docs

| # | Change | Status | Where it goes |
|---|---|---|---|
| D1 | Fidelity benchmark: Gemini writes questions only (no expected answers, no rules); a person writes the attribute rules during review | Agreed in discussion, not applied | SPEC §8, IMPLEMENTATION_PLAN M5 item 1, TEST_PLAN S13 |
| D2 | Census realism check: personas answer held-out factual questions (home ownership, dwelling type, commute mode, language spoken at home…) and are scored against real 2021 Census PUMF answers for their demographic cells; variables used to build personas are excluded | Proposed, awaiting approval | SPEC §8, TEST_PLAN S13, `tools/build-populations` (the same microdata already feeds the US table) |
| D3 | Label-free validity checks (variance, midpoint rate, order effects, run-to-run stability) run on every question, including ones without rules | Part of D1/D2 | TEST_PLAN S13 |
| D4 | The shared spec doc on claude.ai is behind the repo's `docs/SPEC.md` | Not synced since the Gemini-only decision | Re-sync or retire the shared doc in favour of the repo |

## Open decisions (need an answer before the stage that uses them)

| Decision | Needed by | Current assumption |
|---|---|---|
| Gemini usage tier (sets RPM, TPM, RPD defaults) | M2 live runs at scale | Conservative defaults: 60 requests/min, 1M input tokens/min, 1,000 requests/day, concurrency 4 |
| Survey length presets per research type | M3 | 8–10 core questions + 6 suggestions |
| Keep Windows 10 as a supported target | M5 | Yes, while Microsoft extended support lasts |
| Stop & Save final or resumable | M3 | Final |
| Synthesis automatic or on demand | M4 | Automatic on completion |
| Cloud and budget for Windows 11 release VMs | M5 | Azure, started only for release runs |
| Rotate the Gemini key shared in chat; add it as the `GEMINI_API_KEY` repository secret | Now: the M2 live exit check (`live_cohort_200_matches_quotas`) runs only in the `live` CI job | Not confirmed |
