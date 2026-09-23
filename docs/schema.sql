PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;

CREATE TABLE projects (
    id            INTEGER PRIMARY KEY,
    title         TEXT NOT NULL,
    research_goal TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE cohorts (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    config_json TEXT NOT NULL,              -- CohortConfig: N, seed, quotas, screening
    status      TEXT NOT NULL DEFAULT 'draft'
                CHECK (status IN ('draft','generating','ready','locked','failed')),
    parent_cohort_id INTEGER REFERENCES cohorts(id),  -- set when a locked cohort is edited
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE respondents (
    id                    INTEGER PRIMARY KEY,
    cohort_id             INTEGER NOT NULL REFERENCES cohorts(id) ON DELETE CASCADE,
    ordinal               INTEGER NOT NULL,          -- 1..N within the cohort
    quota_cell            TEXT NOT NULL,             -- e.g. "age=25-34|gender=F|region=NE"
    display_name          TEXT,
    age                   INTEGER CHECK (age BETWEEN 13 AND 110),
    gender                TEXT,
    occupation            TEXT,
    income_bracket        TEXT,
    location              TEXT,
    psychographic_summary TEXT NOT NULL,
    persona_json          TEXT NOT NULL CHECK (json_valid(persona_json)),  -- full enrichment incl. biases
    screen_status         TEXT NOT NULL DEFAULT 'passed'
                          CHECK (screen_status IN ('passed','failed','flagged')),
    created_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (cohort_id, ordinal)
);

CREATE TABLE surveys (
    id          INTEGER PRIMARY KEY,
    project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    intro_text  TEXT,                        -- shown to respondents before Q1
    version     INTEGER NOT NULL DEFAULT 1,
    status      TEXT NOT NULL DEFAULT 'draft'
                CHECK (status IN ('draft','in_review','approved')),
    brief_json  TEXT CHECK (brief_json IS NULL OR json_valid(brief_json)),  -- SurveyBrief given to the generator
    generation_model          TEXT,          -- null for fully hand-written surveys
    generation_prompt_version TEXT,
    approved_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE questions (
    id             INTEGER PRIMARY KEY,
    survey_id      INTEGER NOT NULL REFERENCES surveys(id) ON DELETE CASCADE,
    order_index    INTEGER NOT NULL,
    code           TEXT NOT NULL,            -- short stable label, e.g. "Q3_PRICE"
    question_text  TEXT NOT NULL,
    question_type  TEXT NOT NULL
                   CHECK (question_type IN ('single_choice','multi_choice','likert','numeric','open_ended')),
    options_json   TEXT CHECK (options_json IS NULL OR json_valid(options_json)),
        -- choice:  {"options":[{"code":"A","label":"..."}], "randomize":true, "min":1, "max":3}
        -- likert:  {"min":1, "max":7, "labels":{"1":"Strongly disagree","7":"Strongly agree"}}
        -- numeric: {"min":0, "max":10000, "unit":"USD"}
    skip_logic_json TEXT CHECK (skip_logic_json IS NULL OR json_valid(skip_logic_json)),
    is_active      INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1)),
    origin         TEXT NOT NULL DEFAULT 'human'
                   CHECK (origin IN ('ai','ai_edited','human')),
    review_status  TEXT NOT NULL DEFAULT 'pending'
                   CHECK (review_status IN ('pending','accepted','rejected')),
    objective      TEXT,                     -- research objective this question serves (from the brief)
    rationale      TEXT,                     -- generator's reason for asking; never shown to respondents
    original_json  TEXT CHECK (original_json IS NULL OR json_valid(original_json)),  -- AI draft before human edits
    reviewed_at    TEXT,
    UNIQUE (survey_id, order_index),
    UNIQUE (survey_id, code)
);

CREATE TABLE simulation_runs (
    id              INTEGER PRIMARY KEY,
    project_id      INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    survey_id       INTEGER NOT NULL REFERENCES surveys(id),
    cohort_id       INTEGER NOT NULL REFERENCES cohorts(id),
    survey_hash     TEXT NOT NULL,           -- hash of active questions at start
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    temperature     REAL NOT NULL,
    top_p           REAL,
    answer_mode     TEXT NOT NULL CHECK (answer_mode IN ('whole_survey','conversational','independent')),
    prompt_version  TEXT NOT NULL,
    seed            INTEGER NOT NULL,
    max_concurrency INTEGER NOT NULL,
    status          TEXT NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued','running','paused','cancelled','completed','failed')),
    est_cost_usd    REAL,
    started_at      TEXT,
    finished_at     TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE llm_calls (
    id              INTEGER PRIMARY KEY,
    run_id          INTEGER REFERENCES simulation_runs(id) ON DELETE CASCADE,
    cohort_id       INTEGER REFERENCES cohorts(id) ON DELETE CASCADE,  -- set for Phase 1 calls
    respondent_id   INTEGER REFERENCES respondents(id) ON DELETE CASCADE,
    purpose         TEXT NOT NULL CHECK (purpose IN ('persona','survey_draft','answer','theme','critic')),
    attempt         INTEGER NOT NULL DEFAULT 1,
    http_status     INTEGER,
    input_tokens    INTEGER,
    cached_tokens   INTEGER,
    output_tokens   INTEGER,
    latency_ms      INTEGER,
    error           TEXT,
    request_json    TEXT,                    -- stored only when "keep raw prompts" is on
    response_json   TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE TABLE responses (
    id                 INTEGER PRIMARY KEY,
    run_id             INTEGER NOT NULL REFERENCES simulation_runs(id) ON DELETE CASCADE,
    question_id        INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    respondent_id      INTEGER NOT NULL REFERENCES respondents(id) ON DELETE CASCADE,
    llm_call_id        INTEGER REFERENCES llm_calls(id) ON DELETE SET NULL,
    answer_json        TEXT CHECK (answer_json IS NULL OR json_valid(answer_json)),
        -- single: {"code":"B"}  multi: {"codes":["A","D"]}  likert/numeric: {"value":5}  open: {"text":"..."}
    answer_value       REAL,                 -- numeric/likert value, for fast aggregates
    answer_code        TEXT,                 -- single-choice code, for fast aggregates
    reasoning          TEXT,
    shown_options_json TEXT,                 -- option order actually presented
    option_probs_json  TEXT,                 -- per-option probabilities when logprobs are available
    status             TEXT NOT NULL DEFAULT 'valid'
                       CHECK (status IN ('valid','invalid','skipped','refused')),
    created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE (run_id, question_id, respondent_id)
);

CREATE TABLE themes (
    id          INTEGER PRIMARY KEY,
    run_id      INTEGER NOT NULL REFERENCES simulation_runs(id) ON DELETE CASCADE,
    question_id INTEGER NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
    label       TEXT NOT NULL,
    description TEXT
);

CREATE TABLE response_themes (
    response_id INTEGER NOT NULL REFERENCES responses(id) ON DELETE CASCADE,
    theme_id    INTEGER NOT NULL REFERENCES themes(id) ON DELETE CASCADE,
    PRIMARY KEY (response_id, theme_id)
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL                      -- never the API key; that lives in the OS keychain
);

-- A run may only start on an approved survey whose active questions were all accepted by a person.
CREATE TRIGGER trg_runs_require_approved_survey
BEFORE INSERT ON simulation_runs
WHEN (SELECT status FROM surveys WHERE id = NEW.survey_id) IS NOT 'approved'
  OR EXISTS (SELECT 1 FROM questions
             WHERE survey_id = NEW.survey_id AND is_active = 1 AND review_status <> 'accepted')
BEGIN
    SELECT RAISE(ABORT, 'survey_not_approved');
END;

-- Editing an approved survey's questions sends it back to review.
CREATE TRIGGER trg_question_edit_reopens_survey
AFTER UPDATE OF question_text, question_type, options_json, skip_logic_json, is_active ON questions
BEGIN
    UPDATE surveys SET status = 'in_review', approved_at = NULL
    WHERE id = NEW.survey_id AND status = 'approved';
END;

CREATE INDEX idx_respondents_cohort   ON respondents(cohort_id);
CREATE INDEX idx_questions_survey     ON questions(survey_id, order_index);
CREATE INDEX idx_runs_project         ON simulation_runs(project_id, created_at);
CREATE INDEX idx_responses_run_q      ON responses(run_id, question_id);
CREATE INDEX idx_responses_respondent ON responses(respondent_id);
CREATE INDEX idx_llm_calls_run        ON llm_calls(run_id);
CREATE INDEX idx_themes_run_q         ON themes(run_id, question_id);
