-- Development/test fixture: weekly content (prep and reminders) for ISO week 2026-W44.
-- Load AFTER fixtures/2026-w44.sql into a migrated database:
--   sous migrate && sqlite3 sous.db < fixtures/2026-w44.sql && sqlite3 sous.db < fixtures/2026-w44-weekly.sql
-- Rows are inserted out of order on purpose; positions decide the order.
-- Reminders: Wed and Sun have one; Mon has a body and icon but no title (so none); Tue has none.

PRAGMA foreign_keys = ON;
BEGIN;

UPDATE weeks SET prep_intro = 'Monday covers Mon–Wed. Wednesday is a smaller session for Thu–Sun.' WHERE id = '00000001-0000-4000-8000-000000000001';
UPDATE days SET reminder_title = 'Shop day', reminder_body = 'Pick up the Thursday top-up list after work.', reminder_icon = '🧾' WHERE id = '00000002-0000-4000-8000-000000000004';
UPDATE days SET reminder_title = 'Plan next week' WHERE id = '00000002-0000-4000-8000-000000000005';
UPDATE days SET reminder_body = 'Orphan body without a title', reminder_icon = '❓' WHERE id = '00000002-0000-4000-8000-000000000002';

INSERT INTO prep_sessions (id, week_id, position, title, covers) VALUES ('00000008-0000-4000-8000-000000000032', '00000001-0000-4000-8000-000000000001', 10, 'Wednesday Evening Prep (~45 min)', NULL);
INSERT INTO prep_sessions (id, week_id, position, title, covers) VALUES ('00000008-0000-4000-8000-000000000035', '00000001-0000-4000-8000-000000000001', 0, 'Monday Prep Session (~1.5 hours)', 'Covers Monday through Wednesday');

INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('00000009-0000-4000-8000-000000000033', '00000008-0000-4000-8000-000000000032', 5, 'Lentil & Roasted Veg Salad (3 portions)', '35 min (mostly air fryer + simmer)', 'Cook 150g lentils ~20 min; air-fry the veg at 200°C for 15 min; combine and dress.');
INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('00000009-0000-4000-8000-000000000034', '00000008-0000-4000-8000-000000000032', 0, 'Overnight Oats × 1 jar', '3 min', 'One jar: 60g oats + 200ml oat milk + 1 tbsp chia.');
INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('00000009-0000-4000-8000-000000000036', '00000008-0000-4000-8000-000000000035', 20, 'Cook Brown Rice', '25 min', 'Cook 200g dry brown rice; store.');
INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('00000009-0000-4000-8000-000000000037', '00000008-0000-4000-8000-000000000035', 0, 'Start the Gravlax', '15 min', 'Pack the cure over the salmon, wrap tightly and weight it. Flip once a day.');
INSERT INTO prep_tasks (id, session_id, position, name, time, instructions) VALUES ('00000009-0000-4000-8000-000000000038', '00000008-0000-4000-8000-000000000035', 10, 'Air-Fryer Roasted Chickpeas', '20 min', 'Toss chickpeas with oil and paprika; air-fry at 200°C for 12–15 min.');

COMMIT;
