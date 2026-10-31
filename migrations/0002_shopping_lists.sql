-- Feature 003: shopping lists, categories, items and their day links.
-- Ids are canonical lowercase UUID text.

CREATE TABLE shopping_lists (
    id        TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    week_id   TEXT NOT NULL REFERENCES weeks (id) ON DELETE CASCADE,
    position  INTEGER NOT NULL CHECK (position >= 0),
    label     TEXT NOT NULL,
    note      TEXT,
    UNIQUE (week_id, position)
) STRICT;

CREATE TABLE day_shopping_lists (
    day_id            TEXT NOT NULL REFERENCES days (id) ON DELETE CASCADE,
    shopping_list_id  TEXT NOT NULL REFERENCES shopping_lists (id) ON DELETE CASCADE,
    position          INTEGER NOT NULL CHECK (position >= 0),
    PRIMARY KEY (day_id, shopping_list_id),
    UNIQUE (day_id, position)
) STRICT;

CREATE INDEX day_shopping_lists_list ON day_shopping_lists (shopping_list_id);

CREATE TABLE shopping_categories (
    id                TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    shopping_list_id  TEXT NOT NULL REFERENCES shopping_lists (id) ON DELETE CASCADE,
    position          INTEGER NOT NULL CHECK (position >= 0),
    name              TEXT NOT NULL,
    UNIQUE (shopping_list_id, position)
) STRICT;

CREATE TABLE shopping_items (
    id           TEXT PRIMARY KEY NOT NULL CHECK (id GLOB '[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f]-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]'),
    category_id  TEXT NOT NULL REFERENCES shopping_categories (id) ON DELETE CASCADE,
    position     INTEGER NOT NULL CHECK (position >= 0),
    text         TEXT NOT NULL,
    UNIQUE (category_id, position)
) STRICT;

-- A day may only link lists of its own week. A missing row is left to the foreign key.
CREATE TRIGGER day_shopping_lists_same_week_insert
BEFORE INSERT ON day_shopping_lists
WHEN (SELECT week_id FROM days WHERE id = NEW.day_id)
     <> (SELECT week_id FROM shopping_lists WHERE id = NEW.shopping_list_id)
BEGIN
    SELECT RAISE(ABORT, 'day and shopping list belong to different weeks');
END;

CREATE TRIGGER day_shopping_lists_same_week_update
BEFORE UPDATE OF day_id, shopping_list_id ON day_shopping_lists
WHEN (SELECT week_id FROM days WHERE id = NEW.day_id)
     <> (SELECT week_id FROM shopping_lists WHERE id = NEW.shopping_list_id)
BEGIN
    SELECT RAISE(ABORT, 'day and shopping list belong to different weeks');
END;

CREATE TRIGGER days_keep_shopping_links_in_week
BEFORE UPDATE OF week_id ON days
WHEN EXISTS (
    SELECT 1 FROM day_shopping_lists l JOIN shopping_lists s ON s.id = l.shopping_list_id
    WHERE l.day_id = OLD.id AND s.week_id <> NEW.week_id
)
BEGIN
    SELECT RAISE(ABORT, 'day is linked to a shopping list of another week');
END;

CREATE TRIGGER shopping_lists_keep_day_links_in_week
BEFORE UPDATE OF week_id ON shopping_lists
WHEN EXISTS (
    SELECT 1 FROM day_shopping_lists l JOIN days d ON d.id = l.day_id
    WHERE l.shopping_list_id = OLD.id AND d.week_id <> NEW.week_id
)
BEGIN
    SELECT RAISE(ABORT, 'shopping list is linked to a day of another week');
END;
