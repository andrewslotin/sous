# 3. Shopping lists

## Outcome

Display categorized shopping lists on their assigned days, with the reference card styling and ordinary bullet items.

Depends on: [2. Calendar navigation and archive](002-calendar-navigation.md).

## Scope

- Add migrations for `shopping_lists`, `day_shopping_lists`, `shopping_categories`, and `shopping_items` as defined in the [database contract](../technical-design.md#database-contract).
- Extend the week query/document with ordered shopping lists, nested categories/items, and each day's ordered list IDs.
- Display assigned cards above the daily nutrition summary, including list label, optional note, and expandable categories.
- Preserve the pink card styling. Replace checkbox rows with bullets; omit completion counts, checked backgrounds, and strikethrough state.
- Keep item quantities as plain text without parsing or consolidation.

## Relationships and empty states

A shopping list belongs to one week. It can be associated with multiple days of that week; a day can show multiple lists. Card order follows the day association's position. Categories and items follow their own sibling positions.

Prevent cross-week associations on insertion and update, including moves of referenced days or lists to another week. Parent deletions cascade to owned content and association records. Unassigned lists remain in the week document but do not appear on a day.

A day with no assigned lists shows no shopping card. An empty list remains expandable with its label/note and a short “No shopping items.” message. Empty categories retain their headings without placeholder items.

## Acceptance criteria

1. A fixture with two lists on one day renders both in the specified order.
2. One list linked to two days renders the same content on both days without duplicating stored items.
3. Notes, categories, and item text match stored data; updates appear on refresh.
4. No checkbox, completion counter, or checked-item state remains.
5. Invalid references, duplicate sibling positions, duplicate day/list links, and cross-week links are rejected, including moves of linked records.
6. Deleting a list removes its categories, items, and day associations without affecting other lists or days.
7. Empty lists/categories render as specified, and card expansion works with keyboard and pointer input.

## Verification

Extend temporary-database tests for schema constraints, cascades, association ordering, and shared lists. Compare collapsed and expanded shopping cards against the reference, allowing only the agreed bullet-list changes.
