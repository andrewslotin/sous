-- Feature 004: prep sessions, strategy cards and day reminders.
-- Ids are canonical lowercase UUID text. A reminder exists iff days.reminder_title is not null.

ALTER TABLE weeks ADD COLUMN prep_intro TEXT;
ALTER TABLE days ADD COLUMN reminder_title TEXT;
ALTER TABLE days ADD COLUMN reminder_body TEXT;
ALTER TABLE days ADD COLUMN reminder_icon TEXT;

CREATE TABLE prep_sessions (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    week_id TEXT NOT NULL REFERENCES weeks (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    title     TEXT NOT NULL,
    covers    TEXT,
    UNIQUE (week_id, position)
) STRICT;

CREATE TABLE prep_tasks (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    session_id TEXT NOT NULL REFERENCES prep_sessions (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    name          TEXT NOT NULL,
    time          TEXT NOT NULL,
    instructions  TEXT NOT NULL,
    UNIQUE (session_id, position)
) STRICT;

CREATE TABLE strategy_cards (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    week_id TEXT NOT NULL REFERENCES weeks (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    title     TEXT NOT NULL,
    icon      TEXT,
    theme     TEXT NOT NULL CHECK (theme IN ('green', 'blue', 'purple', 'yellow')),
    UNIQUE (week_id, position)
) STRICT;

CREATE TABLE strategy_items (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    card_id TEXT NOT NULL REFERENCES strategy_cards (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    text      TEXT NOT NULL,
    UNIQUE (card_id, position)
) STRICT;
