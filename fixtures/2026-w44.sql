-- Development/test fixture: ISO week 2026-W44 (Mon 2026-10-26 .. Sun 2026-11-01).
-- Meal content adapted from docs/examples/menu.html.
-- Load into a migrated database:
--   sous migrate && sqlite3 sous.db < fixtures/2026-w44.sql
-- Wed has a day row with no meals; Fri and Sat have no day rows.

PRAGMA foreign_keys = ON;
BEGIN;

INSERT INTO weeks (id, iso_year, iso_week, subtitle) VALUES ('00000001-0000-4000-8000-000000000001', 2026, 44, 'Salmon, oats and a little gravlax');

INSERT INTO days (id, week_id, weekday) VALUES ('00000002-0000-4000-8000-000000000002', '00000001-0000-4000-8000-000000000001', 1);
INSERT INTO days (id, week_id, weekday) VALUES ('00000002-0000-4000-8000-000000000003', '00000001-0000-4000-8000-000000000001', 2);
INSERT INTO days (id, week_id, weekday) VALUES ('00000002-0000-4000-8000-000000000004', '00000001-0000-4000-8000-000000000001', 3);
INSERT INTO days (id, week_id, weekday) VALUES ('00000002-0000-4000-8000-000000000005', '00000001-0000-4000-8000-000000000001', 7);

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-000000000006', '00000002-0000-4000-8000-000000000002', 30, 'Snack', 'Walnuts', 'A small handful', 200, 5, 18, 3, 2, 0, '1 min', NULL, 'A crunchy snack.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000007', '00000003-0000-4000-8000-000000000006', 0, 'Portion 30g.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-000000000008', '00000002-0000-4000-8000-000000000002', 20, 'Dinner', 'Miso-Glazed Salmon with Sesame Greens & Rice', 'Per person: 150g salmon, miso glaze, greens, rice', 560, 38, 22.4, 52, 6.5, 1, NULL, '25 min', 'Broil the glaze briefly so it does not burn.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000009', '00000003-0000-4000-8000-000000000008', 4, 'Serve over rice.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000000a', '00000003-0000-4000-8000-000000000008', 3, 'Wilt the greens with sesame oil.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000000b', '00000003-0000-4000-8000-000000000008', 2, 'Bake at 200°C for 10 min.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000000c', '00000003-0000-4000-8000-000000000008', 1, 'Brush on salmon.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000000d', '00000003-0000-4000-8000-000000000008', 0, 'Mix miso, mirin and ginger.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-00000000000e', '00000002-0000-4000-8000-000000000002', 10, 'Lunch', 'Okroshka (option A) / Spinach & Chickpea Salad (option B)', 'Cold kefir soup with cucumber, radish and dill; a chickpea salad for two', 380.5, 16.25, 16, 42, 10, 1, '10 min', NULL, NULL);
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000000f', '00000003-0000-4000-8000-00000000000e', 1, 'Combine with kefir and season.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000010', '00000003-0000-4000-8000-00000000000e', 0, 'Chop cucumber, radish and dill.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-000000000011', '00000002-0000-4000-8000-000000000002', 0, 'Breakfast', 'Overnight Oats — Apple & Walnut', '60g oats, 200ml oat milk, 30g walnuts, grated apple, chia, cinnamon', 430, 14, 18, 52, 10, 0, '2 min', NULL, 'Make it the night before.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000012', '00000003-0000-4000-8000-000000000011', 2, 'Top with grated apple and walnuts.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000013', '00000003-0000-4000-8000-000000000011', 1, 'Refrigerate overnight.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000014', '00000003-0000-4000-8000-000000000011', 0, 'Stir oats, milk and chia in a jar.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-000000000015', '00000002-0000-4000-8000-000000000003', 10, 'Dinner', 'Salmon en Papillote with Lemon, Dill & Courgette', 'Salmon parcels with lemon slices and dill', 460, 38, 22, 14, 4, 1, NULL, '30 min (mostly oven)', 'Open the parcel at the table.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000016', '00000003-0000-4000-8000-000000000015', 1, 'Bake 15 min at 190°C.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000017', '00000003-0000-4000-8000-000000000015', 0, 'Fold baking paper into parcels.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-000000000018', '00000002-0000-4000-8000-000000000003', 0, 'Breakfast', 'Skyr Bowl with Berries & Almonds', 'Skyr, mixed berries, almonds', 320, 26, 10, 30, 4, 0, '3 min', NULL, NULL);
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-000000000019', '00000003-0000-4000-8000-000000000018', 1, 'Top with berries and almonds.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000001a', '00000003-0000-4000-8000-000000000018', 0, 'Spoon skyr into a bowl.');

INSERT INTO meals (id, day_id, position, type, name, description, kcal, protein_g, fat_g, carbs_g, fiber_g, for_two, assembly_time, cook_time, tip)
VALUES ('00000003-0000-4000-8000-00000000001b', '00000002-0000-4000-8000-000000000005', 0, 'Lunch', 'Manchego, Apple & Walnut Salad', 'Per person: mixed leaves, ~30g shaved Manchego, ½ apple, 20g walnuts, lemon-olive oil', 380, 14, 24, 24, 6, 1, '8 min', NULL, 'Manchego as an accent — ~30g each.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000001c', '00000003-0000-4000-8000-00000000001b', 2, 'Dress with olive oil, lemon and Dijon.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000001d', '00000003-0000-4000-8000-00000000001b', 1, 'Shave Manchego over each plate.');
INSERT INTO meal_steps (id, meal_id, position, text) VALUES ('00000004-0000-4000-8000-00000000001e', '00000003-0000-4000-8000-00000000001b', 0, 'Bed of mixed leaves. Sliced apple, toasted walnuts.');

COMMIT;
