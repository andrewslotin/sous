# Sous: technical design

Status: agreed product scope; implementation design proposed below.

Service name: **Sous**. Repository and binary name: `sous`.

## Purpose and scope

Provide a small, single-household HTTP service that stores weekly meal-plan content in SQLite and displays it using the design in [`docs/examples/menu.html`](examples/menu.html). Application authentication is not required.

V1 serves existing data. Another tool or process populates the database, outside this project's scope. Changes become visible on the next page load after the writing transaction commits.

Included:

- Current-week and permanent week pages.
- Daily meals and nutrition summaries, shopping lists, prep sessions, and strategy cards.
- An archive listing available weeks and useful missing-plan pages.
- A documented SQLite schema and read-only HTTP contract.

Excluded:

- Data-entry UI, import tools, public write API, and meal generation.
- Draft/publish workflow, accounts, shopping checkmarks, live updates, and offline mode.
- Ingredient catalogs, recipe reuse, nutrition calculation from ingredients, and automatic shopping-list generation.

## Architecture

```text
Browser → HTTP service
             ├── page shell and bundled React assets
             ├── read-only HTTP data endpoints
             └── application queries → SQLite
```

The application serves frontend assets and data from the same origin and reads from one SQLite database. Separate transport handlers, application queries, and persistence code. Backend language and SQLite library remain implementation choices; they must support transactions and foreign keys.

## Calendar and routing

- Use ISO 8601 week numbering: Monday–Sunday; week 1 contains January 4.
- Route year means ISO week-year, not necessarily the year of every date in that week.
- Configure the service timezone with `APP_TIMEZONE`, default `Europe/Berlin`.
- Determine today's date and current week on the server. Browser timezone must not alter the selected week or day.
- Store dates as `YYYY-MM-DD` strings; do not interpret them as UTC instants in the browser.

| Route | Behavior |
| --- | --- |
| `/` | Render current ISO week using configured timezone. |
| `/<year>/<week>` | Render the specified ISO week; canonical week number has no leading zero. |
| `/weeks` | List stored plans in descending week order, with date ranges and links. |
| `/api/weeks/<year>/<week>` | Return a complete, ordered week document as JSON. |

For example, `/2026/44` represents October 26–November 1, 2026. Validate that the requested week exists in the ISO calendar; some years have no week 53. Accept four-digit years from 0001–9999 only when the entire seven-day date range is representable. Redirect zero-padded valid week paths to their canonical form.

The root route retains `/` in the address bar. On each navigation or reload it resolves the current week again. Explicit week URLs always retain their requested week, regardless of today's date.

On the current week's page, initially select today. On any other week's page, select Monday. Day controls use Monday–Sunday order. Changing tabs or days does not fetch a different week.

## Missing data and errors

A plan exists when a `weeks` record exists. An empty database is a supported first-run state.

- Missing plan at `/`: HTTP 200 with the requested date range, “No meal plan for this week yet.”, and links to available weeks.
- Missing plan at a valid explicit week URL: HTTP 404 with the same useful content.
- Invalid year/week route: HTTP 404 with “Invalid week.” and a link to the current week and archive.
- No plans anywhere: show “No meal plans available yet.” instead of an empty archive list.
- Existing week with no meals for a day: show “No meals planned for this day.” and omit the nutrition summary. Do not invent zero-valued meals.
- No prep sessions or strategy cards: show a short empty message in the respective existing tab.
- Database failure: HTTP 503 and a generic availability message. Never present a database failure as a missing plan; log details on the server.

The HTML response itself has the correct status; do not serve HTTP 200 for every path and defer missing-plan detection entirely to the browser.

## Interface preservation

[`docs/examples/menu.html`](examples/menu.html) is the visual reference. Retain its 540px maximum content width, background, spacing, fonts, gradients, colors, borders, rounded cards, badges, and responsive behavior.

Retain:

- Daily Plan, Prep Sessions, and Strategy tabs.
- Day selector and expandable meal, prep, and shopping cards.
- Meal descriptions, nutrition values, ordered instructions, and tip callouts.
- Daily nutrition totals and the existing meal-type colors and icons.
- Shopping-list categories and notes; multiple lists can appear on a day in stored order.
- Strategy card appearance using four predefined themes: green, blue, purple, and yellow.

Intentional changes:

- Shopping items become ordinary bullet lists. Remove checkbox controls, completion counts, checked colors, and strikethrough state.
- Display days in ISO week order.
- Add a discreet “All weeks” link and archive/missing-plan pages using the existing typography and container.
- Select today only when viewing the actual current week; the sample selects by weekday even for old plans.

Move all week-specific text into the database, including the header subtitle, prep introduction, strategy cards, and optional day reminder. Keep “Meal Plan”, tab names, empty-state messages, and nutrition labels as UI text.

Continue using the existing React component structure and styles. Compile JSX at build time instead of loading Babel in the browser. Bundle React and self-host the reference fonts so page rendering does not depend on public CDNs. Match the existing body font fallback behavior rather than introducing a new font.

Expanded cards and selected tabs are transient browser state. No persisted UI state or shopping progress is required. Text from the database is rendered as escaped text, never executable HTML. Preserve keyboard access to tabs and expandable controls without changing their appearance.

## Database contract

Use relational tables with stable UUID text IDs for independently addressable content. Keep IDs stable when editing or reordering existing records. Week identity is the unique `(iso_year, iso_week)` pair.

All child collections have an integer `position >= 0`, unique within their parent. Positions need not be contiguous. Queries always order explicitly by position. Required text columns are non-null; optional text uses NULL. No stored HTML, JavaScript, or arbitrary CSS.

| Table | Fields and constraints |
| --- | --- |
| `schema_migrations` | Applied migration version and application timestamp; managed by the service only. |
| `weeks` | `id` PK, `iso_year`, `iso_week`, optional `subtitle`, optional `prep_intro`; unique year/week; application/writer validates ISO week existence. |
| `days` | `id` PK, `week_id` FK, `weekday` integer 1–7 (Monday=1), optional `reminder_title`, optional `reminder_body`, optional `reminder_icon`; unique week/weekday. |
| `meals` | `id` PK, `day_id` FK, `position`, `type` enum Breakfast/Lunch/Dinner/Snack, `name`, `description`, `kcal`, `protein_g`, `fat_g`, `carbs_g`, `fiber_g`, `for_two` boolean, optional `assembly_time`, optional `cook_time`, optional `tip`. |
| `meal_steps` | `id` PK, `meal_id` FK, `position`, `text`. |
| `shopping_lists` | `id` PK, `week_id` FK, `position`, `label`, optional `note`. |
| `day_shopping_lists` | `day_id` FK, `shopping_list_id` FK, `position`; composite PK day/list; unique day/position. Both records must belong to the same week. |
| `shopping_categories` | `id` PK, `shopping_list_id` FK, `position`, `name`. |
| `shopping_items` | `id` PK, `category_id` FK, `position`, `text`. |
| `prep_sessions` | `id` PK, `week_id` FK, `position`, `title`, optional `covers`. |
| `prep_tasks` | `id` PK, `session_id` FK, `position`, `name`, `time`, `instructions`. |
| `strategy_cards` | `id` PK, `week_id` FK, `position`, `title`, optional `icon`, `theme` enum green/blue/purple/yellow. |
| `strategy_items` | `id` PK, `card_id` FK, `position`, `text`. |

Parent deletion cascades to owned children and association records. Index foreign keys used to assemble a week. Enforce cross-week shopping associations with insert/update triggers, including attempts to move referenced records between weeks.

Week date ranges and day dates derive from ISO year/week and weekday; do not store duplicate date labels. A week may omit day records; the response still contains seven ordered days with empty content for missing records.

Nutrition values are required non-negative numbers. Values represent the displayed individual's daily intake, matching the sample's summation behavior. `for_two` controls only the FOR TWO badge; never multiply nutrition by two. Mixed-person meal descriptions remain free text. Daily totals are derived from stored meal values, not stored separately. Sum before formatting and display at most one decimal place, dropping trailing `.0`.

Times remain display text, such as “35 min (mostly air fryer + simmer)”. Shopping quantities remain part of item text. Do not parse, consolidate, or reinterpret them.

### Data consistency

The schema is a documented integration contract; implementing data entry is outside scope. IDs remain stable across edits. Related changes must commit atomically, so readers see either the previous or committed state, never a partially written plan. Foreign-key and semantic constraints, including valid ISO weeks and supported types/themes, apply to all stored content.

## Read API and rendering

The page handler reads a complete week in one read transaction and embeds its initial JSON document safely into the HTML shell. This avoids a second initial data request and ensures status, headings, dates, and content describe the same snapshot. Escape embedded JSON appropriately so text such as `</script>` cannot terminate its container.

The JSON endpoint exposes the same document shape:

```text
schemaVersion
week: { id, isoYear, isoWeek, startDate, endDate, subtitle, prepIntro }
days: [ { id|null, date, weekday, meals: [...], shoppingListIds: [...], reminder|null } ]
shoppingLists: [ { id, label, note, categories: [ { id, name, items: [...] } ] } ]
prepSessions: [ { id, title, covers, tasks: [...] } ]
strategyCards: [ { id, title, icon, theme, items: [...] } ]
```

Nested meals and tasks carry their stored display fields and IDs. Arrays are ordered. Nullable fields are explicit; absent collections are empty arrays. The API document does not expose internal migration tables or filesystem paths. Missing/invalid weeks return 404 with a stable JSON error code; database failures return 503.

Do not cache HTML or week JSON: send `Cache-Control: no-store`. Serve content-hashed static assets with long-lived immutable caching. No application data cache or restart is needed to observe committed writes. An open page changes only after navigation or refresh.

Request handlers use read-only database connections. Mutation requests to defined page and API routes receive 405. Do not enable broad cross-origin access.

## Verification and acceptance criteria

1. A populated fixture renders all tabs, cards, text, badges, instructions, and strategy themes with the reference styling. Compare desktop and narrow mobile screenshots, including expanded states; documented shopping/day-order/archive changes are expected.
2. `/` resolves the current ISO week in the configured timezone; `/2026/44` shows October 26–November 1, 2026.
3. Test ISO year boundaries, valid/invalid week 53, timezone midnight boundaries, canonical redirects, and old/future week selection.
4. Missing current plans, missing explicit weeks, empty archives, missing days, and unavailable databases produce their specified messages and HTTP statuses.
5. Editing a meal through a separate database connection becomes visible on refresh without restarting. An uncommitted transaction is never partially rendered.
6. Shopping lists render categorized bullets with no checkboxes or progress state. Collection ordering remains stable after edits.
7. Nutrition totals sum stored values once per meal regardless of FOR TWO, and decimal formatting is deterministic.
8. Database constraints reject invalid references, duplicate sibling positions, and cross-week shopping associations.
9. User-supplied text is escaped in both React rendering and initial JSON embedding.
10. Pages render without third-party CDN access.

The sample’s Sunday–Saturday date range spans two ISO weeks. Use an explicitly dated fixture for visual verification rather than silently shifting its dates.
