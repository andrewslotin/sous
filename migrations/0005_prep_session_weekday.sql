-- Feature 013: give each prep session an optional weekday of its own week (Monday=1).
ALTER TABLE prep_sessions ADD COLUMN weekday INTEGER CHECK (weekday BETWEEN 1 AND 7);
