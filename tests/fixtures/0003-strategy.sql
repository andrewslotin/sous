-- Frozen test fixture: the strategy rows that features 4–11 stored for 2026-W44. Loaded only into a database migrated through 0003, before 0004 drops the tables.

INSERT INTO strategy_cards (id, week_id, position, title, icon, theme) VALUES ('0000000a-0000-4000-8000-000000000039', '00000001-0000-4000-8000-000000000001', 30, 'This Week''s Notes', '📝', 'yellow');
INSERT INTO strategy_cards (id, week_id, position, title, icon, theme) VALUES ('0000000a-0000-4000-8000-00000000003b', '00000001-0000-4000-8000-000000000001', 0, 'Weekly Variety', '🐟', 'green');
INSERT INTO strategy_cards (id, week_id, position, title, icon, theme) VALUES ('0000000a-0000-4000-8000-00000000003f', '00000001-0000-4000-8000-000000000001', 20, 'The Salmon Plan', NULL, 'purple');
INSERT INTO strategy_cards (id, week_id, position, title, icon, theme) VALUES ('0000000a-0000-4000-8000-000000000041', '00000001-0000-4000-8000-000000000001', 10, 'Air Fryer Features', '🍳', 'blue');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-00000000003a', '0000000a-0000-4000-8000-000000000039', 0, 'Monday + Thursday shop split');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-00000000003c', '0000000a-0000-4000-8000-00000000003b', 10, 'Legumes daily');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-00000000003d', '0000000a-0000-4000-8000-00000000003b', 0, 'Oily fish 3×');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-00000000003e', '0000000a-0000-4000-8000-00000000003b', 20, 'Nuts daily — walnuts & almonds');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-000000000040', '0000000a-0000-4000-8000-00000000003f', 0, '~300g → gravlax');
INSERT INTO strategy_items (id, card_id, position, text) VALUES ('0000000b-0000-4000-8000-000000000042', '0000000a-0000-4000-8000-000000000041', 0, 'Mon: crispy chickpeas');
