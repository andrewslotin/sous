-- Development/test fixture: earlier weeks for recipe recall (feature 6), ISO weeks 2026-W38 and 2026-W41.
-- Independent of the W44 fixtures. Load into a migrated database:
--   sous migrate && sqlite3 sous.db < fixtures/2026-history.sql
-- Rows are inserted out of order on purpose; positions decide the order.

PRAGMA foreign_keys = ON;
BEGIN;

-- 2026-W41: Monday 2026-10-05 .. Sunday 2026-10-11
INSERT INTO weeks (id, iso_year, iso_week, subtitle, prep_intro) VALUES ('000000c1-0000-4000-8000-000000000041', 2026, 41, 'Autumn comfort food', NULL);
INSERT INTO days (id, week_id, weekday) VALUES ('000000c2-0000-4000-8000-000000000415', '000000c1-0000-4000-8000-000000000041', 5);
INSERT INTO days (id, week_id, weekday) VALUES ('000000c2-0000-4000-8000-000000000413', '000000c1-0000-4000-8000-000000000041', 3);
INSERT INTO days (id, week_id, weekday) VALUES ('000000c2-0000-4000-8000-000000000411', '000000c1-0000-4000-8000-000000000041', 1);

-- Wednesday 2026-10-07: the casserole dinner
INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('000000c3-0000-4000-8000-000000000001', '000000c2-0000-4000-8000-000000000413', 10, 'Dinner', 'Chicken & Broccoli Casserole', 'Per person: 150g chicken, broccoli, light cheese sauce', 610, 45, 21.5, 48, 7, 1, '15 min', '35 min', 'Rest 5 min before serving.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000003', '000000c3-0000-4000-8000-000000000001', 20, 'Layer chicken, broccoli and sauce in the dish.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000001', '000000c3-0000-4000-8000-000000000001', 0, 'Heat the oven to 190°C.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000004', '000000c3-0000-4000-8000-000000000001', 30, 'Bake 35 min until golden.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000002', '000000c3-0000-4000-8000-000000000001', 10, 'Brown the chicken.');

-- Friday 2026-10-09: leftovers (matches "casserole" by description only, in upper case)
INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('000000c3-0000-4000-8000-000000000002', '000000c2-0000-4000-8000-000000000415', 0, 'Lunch', 'Leftover bowl', 'Reheated CASSEROLE with greens', 480, 32, 16, 40, 6, 0, '5 min', NULL, NULL);
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000005', '000000c3-0000-4000-8000-000000000002', 0, 'Reheat and serve over greens.');

-- Monday 2026-10-05: literal SQL wildcard characters in text
INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('000000c3-0000-4000-8000-000000000003', '000000c2-0000-4000-8000-000000000411', 0, 'Breakfast', '100% Rye Toast', 'Rye toast with cottage_cheese', 350, 20, 9, 45, 8, 0, '5 min', NULL, NULL);

-- 2026-W38: Monday 2026-09-14 .. Sunday 2026-09-20
INSERT INTO weeks (id, iso_year, iso_week, subtitle, prep_intro) VALUES ('000000c1-0000-4000-8000-000000000038', 2026, 38, NULL, NULL);
INSERT INTO days (id, week_id, weekday) VALUES ('000000c2-0000-4000-8000-000000000383', '000000c1-0000-4000-8000-000000000038', 3);

-- Wednesday 2026-09-16: the same dish, an earlier copy with its own id and steps
INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('000000c3-0000-4000-8000-000000000004', '000000c2-0000-4000-8000-000000000383', 10, 'Dinner', 'Chicken & Broccoli Casserole', 'Per person: 150g chicken, broccoli, light cheese sauce', 600, 44, 21, 48, 7, 1, '15 min', '40 min', NULL);
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000007', '000000c3-0000-4000-8000-000000000004', 1, 'Bake 40 min.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('000000c4-0000-4000-8000-000000000006', '000000c3-0000-4000-8000-000000000004', 0, 'Mix everything in the dish.');

-- Wednesday 2026-09-16 lunch: non-ASCII case folding
INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('000000c3-0000-4000-8000-000000000005', '000000c2-0000-4000-8000-000000000383', 0, 'Lunch', 'Käsespätzle', 'With ÄPFEL compote', 650, 25, 28, 70, 5, 0, NULL, '20 min', NULL);

COMMIT;
