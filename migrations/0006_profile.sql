-- Feature 017: the household profile, five fixed plain-text sections independent of weekly plans.
CREATE TABLE profile_sections (
    key   TEXT PRIMARY KEY NOT NULL CHECK (key IN ('household', 'food', 'kitchen', 'planning', 'other')),
    text  TEXT NOT NULL
) STRICT;

INSERT INTO profile_sections (key, text) VALUES
    ('household', ''),
    ('food', ''),
    ('kitchen', ''),
    ('planning', ''),
    ('other', '');
