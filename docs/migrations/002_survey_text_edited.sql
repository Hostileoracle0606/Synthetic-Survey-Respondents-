-- B16: the survey title and respondent intro are editable in Step 3. Redraft keeps a
-- person's wording, so the survey records whether it was edited by hand.
ALTER TABLE surveys ADD COLUMN text_edited INTEGER NOT NULL DEFAULT 0 CHECK (text_edited IN (0,1));
