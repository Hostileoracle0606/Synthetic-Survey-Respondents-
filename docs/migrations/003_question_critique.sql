-- B9: the critic's flags are stored with the question they judge, as a `Critique`:
-- {"status":"checking"|"done"|"failed","flags":[{"issue":"leading","note":"..."}],"error":null,"promptVersion":"critic.v1"}
-- Cleared when the question's wording changes. Advice only: approval never reads it.
ALTER TABLE questions ADD COLUMN critic_json TEXT CHECK (critic_json IS NULL OR json_valid(critic_json));
