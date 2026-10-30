-- Feature 001: weeks, days, meals and meal steps.
-- Ids are canonical lowercase UUID text.

CREATE TABLE weeks (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    iso_year  INTEGER NOT NULL CHECK (iso_year BETWEEN 1 AND 9999),
    iso_week  INTEGER NOT NULL CHECK (iso_week BETWEEN 1 AND 53),
    subtitle  TEXT,
    UNIQUE (iso_year, iso_week)
) STRICT;

-- ISO year Y has a week 53 when 1 January is a Thursday, or a Wednesday in a leap year.
CREATE TRIGGER weeks_valid_iso_week_insert
BEFORE INSERT ON weeks
WHEN NEW.iso_week = 53 AND NOT (
    strftime('%w', printf('%04d-01-01', NEW.iso_year)) = '4'
    OR (strftime('%w', printf('%04d-01-01', NEW.iso_year)) = '3'
        AND NEW.iso_year % 4 = 0 AND (NEW.iso_year % 100 <> 0 OR NEW.iso_year % 400 = 0))
)
BEGIN
    SELECT RAISE(ABORT, 'ISO week 53 does not exist in this year');
END;

CREATE TRIGGER weeks_valid_iso_week_update
BEFORE UPDATE OF iso_year, iso_week ON weeks
WHEN NEW.iso_week = 53 AND NOT (
    strftime('%w', printf('%04d-01-01', NEW.iso_year)) = '4'
    OR (strftime('%w', printf('%04d-01-01', NEW.iso_year)) = '3'
        AND NEW.iso_year % 4 = 0 AND (NEW.iso_year % 100 <> 0 OR NEW.iso_year % 400 = 0))
)
BEGIN
    SELECT RAISE(ABORT, 'ISO week 53 does not exist in this year');
END;

-- 9999-W52 ends on 10000-01-02, outside the representable range (0001-01-01 starts 0001-W01).
CREATE TRIGGER weeks_date_range_insert
BEFORE INSERT ON weeks
WHEN NEW.iso_year = 9999 AND NEW.iso_week > 51
BEGIN
    SELECT RAISE(ABORT, 'week is outside the supported date range');
END;

CREATE TRIGGER weeks_date_range_update
BEFORE UPDATE OF iso_year, iso_week ON weeks
WHEN NEW.iso_year = 9999 AND NEW.iso_week > 51
BEGIN
    SELECT RAISE(ABORT, 'week is outside the supported date range');
END;

CREATE TABLE days (
    id       TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    week_id  TEXT NOT NULL REFERENCES weeks (id) ON DELETE CASCADE,
    weekday  INTEGER NOT NULL CHECK (weekday BETWEEN 1 AND 7),
    UNIQUE (week_id, weekday)
) STRICT;

CREATE TABLE meals (
    id             TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    day_id         TEXT NOT NULL REFERENCES days (id) ON DELETE CASCADE,
    position       INTEGER NOT NULL CHECK (position >= 0),
    type           TEXT NOT NULL CHECK (type IN ('Breakfast', 'Lunch', 'Dinner', 'Snack')),
    name           TEXT NOT NULL,
    description    TEXT NOT NULL,
    kcal           REAL NOT NULL CHECK (kcal >= 0),
    protein_g      REAL NOT NULL CHECK (protein_g >= 0),
    fat_g          REAL NOT NULL CHECK (fat_g >= 0),
    carbs_g        REAL NOT NULL CHECK (carbs_g >= 0),
    fiber_g        REAL NOT NULL CHECK (fiber_g >= 0),
    for_two        INTEGER NOT NULL CHECK (for_two IN (0, 1)),
    assembly_time  TEXT,
    cook_time      TEXT,
    tip            TEXT,
    UNIQUE (day_id, position)
) STRICT;

CREATE TABLE meal_steps (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    meal_id   TEXT NOT NULL REFERENCES meals (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    text      TEXT NOT NULL,
    UNIQUE (meal_id, position)
) STRICT;
