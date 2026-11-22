# 20. Shopping checkboxes

## Outcome

Each shopping item gets a checkbox, so the cook can cross items off in the shop with the plan open on their phone. A ticked box fills red, so the empty square becomes the shopping square, and the item is struck through. Ticks are kept on the device. They survive closing the card, reloading the page and coming back another day, and they're never sent to the server.

Depends on: [14. Redesign](014-redesign.md).

## Scope

This changes the shopping list card on day, week and missing-plan pages. Routes, statuses, the page JSON, the API, the MCP tools and the database don't change. Ticks stay on one browser and aren't shared across devices or with the household. Feature 14 removed checkbox controls and strike-through from shopping items, and the technical design excluded shopping checkmarks. After using the app, the household found they need them while shopping. This feature reverses that decision: it brings back the checkbox and the strike-through, but not completion counts.

Out of scope: syncing ticks between devices or tabs, a "clear ticks" action, completion counts, and moving ticked items to the end.

## Behaviour

### Ticking

- Each item in an open shopping card is a native checkbox inside a `<label>` that holds the item's text. The item's text is the checkbox's accessible name.
- Tapping anywhere on the row toggles it, and so does Space when the box has focus. Tab moves through the boxes in list order.
- A ticked item stays where it is. The list keeps the household's order.
- Ticking never opens, closes or reorders a card. The collapsed card's band shows no ticks or counts.

### Keeping ticks

- Ticks are stored in the browser's local storage under `sous-shopping-ticks`, next to the theme choice (`sous-theme`). The value is a JSON object keyed by shopping item id:
  `{ "5a5a0001-…": { "text": "2 red peppers", "at": "2026-11-22" } }`
  `text` is the item's text when it was ticked. `at` is the page's `today` (the server's today in `APP_TIMEZONE`) on the day it was ticked.
- An item shows as ticked only when its id is in the object and the stored `text` equals its current text exactly. If the planner rewrites an item and keeps its id, the tick is dropped, so "2 peppers" ticked doesn't tick "3 peppers". If an item is replaced with a new id, it starts unticked.
- Unticking removes the item's entry.
- One list can show on several days, and its items have the same ids on all of them. Ticking an item on Monday also shows it ticked on Tuesday.
- Ticks expire. When a page with shopping lists loads, entries whose `at` is more than 30 days before the page's `today` are removed. This keeps storage small without a clear action.
- Ticks are read when the page loads. A tick made in another tab shows after a reload.
- If local storage is unavailable, unreadable, full, or holds a value that isn't the expected object, the page still renders. Ticks then work for as long as the page stays open, and no error is shown. A malformed stored value is treated as empty and replaced on the next tick. A single malformed entry is ignored.

## Layout and styles

- The box is an 18px square (`radius-square`) with a 1.5px `ink` ring on `surface`, sitting `space-3` before the item's text and aligned to its first line. On row hover, the box fills `surface-sunken`.
- Ticked, the box fills solid `red` and its ring turns `red`. It has no tick glyph: the filled square is the mark. The item's text turns `ink-muted` with a 1.5px line-through.
- Keyboard focus shows the standard 2px `focus` outline at a 2px offset around the box.
- Each row is a 40px tap target: 9px padding above and below a 22px line. The row's `line` rule and the category layout don't change. A long item wraps under its own text, not under the box.
- The checkbox is styled with `appearance: none` and uses only design-system tokens, in light and in dark.

## Documentation

- [Design system](../design-system.md), written by one owner:
  - **Colour, Red (shopping):** a ticked item's box fills `red`: crossing an item off turns its empty square into the shopping square.
  - **Token table:** add the ticked box to `red`'s use, the checkbox ring to `ink`'s, the ticked item's text to `ink-muted`'s, and the box hover to `surface-sunken`'s.
  - **Focus:** checkboxes join links, buttons and radios.
  - **Shopping list:** each item is a checkbox row, with the box, its states, the struck-through text and the 40px row, as above.
  - **Shopping list**, continued: ticks are kept on the device, like the theme choice, and never leave it.
- [Technical design](../technical-design.md):
  - Remove "shopping checkmarks" from the excluded list, and exclude shared or synced shopping ticks instead.
  - Under feature 14's intentional changes, add that feature 20 brings back a checkbox per item and the strike-through for ticked items.
  - The UI state paragraph: shopping ticks are kept in local storage with the theme choice, as described above. Replace "No shopping progress is required."
  - Acceptance criterion 6: shopping rows have a checkbox each and no progress count.

## Acceptance criteria

1. Every item in an open shopping card is a native checkbox with the item's text as its accessible name. Clicking the row, clicking the box and pressing Space on the focused box each toggle it.
2. A ticked item's box is `red`-filled with a `red` ring. Its text is `ink-muted` with a line-through. An unticked item is unchanged from today, apart from the box. The order of items never changes.
3. Ticks survive closing and reopening the card, reloading the page, and navigating to another day that shows the same list.
4. Changing an item's text, keeping its id, drops its tick. A new item id starts unticked.
5. Entries ticked more than 30 days before the page's `today` are removed on load. Newer ones are kept.
6. With local storage throwing on access, or holding malformed JSON, the page renders and ticking still works until reload.
7. No request is sent to the server when an item is ticked or unticked.
8. The box, its hover, its focus outline and the ticked state follow the design system in light and in dark. Rows are at least 40px tall, and long items wrap beside the box, at 320 px and above.
9. The existing tests pass after being updated. `shopping_items_are_plain_rows` now expects one checkbox per item and still expects no progress count and no bullets.

## Verification

- Browser tests:
  - The checkbox count equals the item count, and each box's accessible name is its item's text.
  - Toggling by row click, box click and Space; the `li` and its computed styles (box background, text colour and decoration) before and after a tick.
  - Ticks after closing and reopening the card, after a reload, and on a second day that shows the same list.
  - After a fixture edit that changes an item's text and keeps its id, the item is unticked after a reload.
  - A seeded `sous-shopping-ticks` value with one entry older than 30 days and one newer: after load, only the newer one is in storage.
  - Local storage replaced by a throwing stub, and then a malformed stored value: the page renders with no console error and ticking works.
  - No network request is made on tick.
  - Row height of at least 40px, and a long item that wraps with its box on the first line.
  - Screenshots at 375 and 1280 px, light and dark, of an open shopping card with some items ticked.
