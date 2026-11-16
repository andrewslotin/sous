# Sous: technical design

Status: agreed product scope; implementation design proposed below.

Service name: **Sous**. Repository and binary name: `sous`.

## Purpose and scope

Provide a small, single-household HTTP service that stores weekly meal-plan content in SQLite and displays it as the [design system](design-system.md) specifies. Application authentication is not required.

V1 serves existing data. Another tool or process populates the database, outside this project's scope. Changes become visible on the next page load after the writing transaction commits.

Included:

- Current-week and permanent week pages.
- Daily meals and nutrition summaries, shopping lists, and prep sessions.
- Year archive pages listing available weeks, and useful missing-plan pages.
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

The application serves frontend assets and data from the same origin and reads from one SQLite database. Use Rust with Axum and Tokio, SQLx for SQLite queries and versioned migrations, and Serde for serialization. Separate transport handlers, application queries, and persistence code, without a generic repository framework. Use runtime-checked queries with temporary-database integration tests; compilation does not require a database or checked-in SQLx query metadata. Compile the existing React frontend at build time and embed its assets in the `sous` binary. The service ships as a statically linked binary in a `scratch` container image.

The [feature tracker](README.md) links the incremental implementation specs. Features 1–5 implement this read-only baseline. [Feature 6: MCP meal planning and recall](features/006-mcp-meal-planning.md) extends it with agent-facing queries and transactional writes at `/mcp`; its contract supersedes the baseline's data-entry exclusion and read-only connection rule only for MCP write tools. Existing page and JSON API contracts remain unchanged.

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
| `/<date>` | Render the ISO week containing the date (`YYYY-MM-DD`), showing that day. |
| `/<year>` | List the stored plans of one ISO week-year in descending week order, with date ranges and links, and link the adjacent years that have plans. |
| `/api/<year>/<week>` | Return a complete, ordered week document as JSON. |
| `/api/<date>` | Return the document of the week containing the date as JSON. |
| `/api/<year>` | Return the year page's listing as JSON. |

For example, `/2026/44` represents October 26–November 1, 2026. Validate that the requested week exists in the ISO calendar; some years have no week 53. Accept four-digit years from 0001–9999 only when the entire seven-day date range is representable. Redirect zero-padded valid week paths to their canonical form.

A year path is exactly four ASCII digits from 0001 to 9999, the same form as the year in week URLs. A single-segment path that is neither a year nor shaped like a date (see below), such as `/weeks` or `/2026-11`, is a plain 404. A year page lists the weeks whose ISO week-year is in its URL, so `/2026/1` (December 29, 2025–January 4, 2026) is on `/2026`. It links to the previous and next year only when that adjacent year has at least one stored plan. The current year is the ISO week-year of today in the configured timezone, resolved on every request. Year pages are still served at `/<year>`, but no page links to them.

The root route retains `/` in the address bar. On each navigation or reload it resolves the current week again. Explicit week URLs always retain their requested week, regardless of today's date.

A date path is exactly `YYYY-MM-DD`: four, two and two ASCII digits, a real calendar date, and its ISO week fully representable. A well-formed but impossible or unrepresentable date (such as `/2026-02-30`) renders the invalid date page (HTTP 404). `/2026-1-5` is not date-shaped, so it is a plain 404.

A plan page shows one day, the page's displayed date (`displayedDate` in the page data). At `/<date>` it is the date. At `/` and `/<year>/<week>` it is today when today is in the week and Monday otherwise. A missing-plan page for a date path also carries its date; invalid pages have none. Previous and next day are ordinary links to date paths, so every day change is a navigation; the first and last representable days have a disabled step. The page has no tabs and no day selector.

## Missing data and errors

A plan exists when a `weeks` record exists. An empty database is a supported first-run state.

- Missing plan at `/`: HTTP 200 with the requested date range, “No meal plan for this week yet.”, and the month calendar. Do not show a list of available weeks.
- Missing plan at a valid explicit week or date URL: HTTP 404 with the same useful content.
- Invalid year/week route: HTTP 404 with “Invalid week.”, a link to the current week and the month calendar. If the invalid page cannot read `plannedWeeks`, the database failure rule below applies (HTTP 503).
- Invalid date route: HTTP 404 with “Invalid date.”, a link to the current week and the month calendar, titled “Invalid date — Meal Plan”.
- Year without plans: show “No meal plans available yet.” and the year links instead of an empty list. HTTP 200 for the current year, including an empty database; HTTP 404 for any other year.
- Existing week with no meals for a day: show “No meals planned for this day.” and omit the nutrition summary. Do not invent zero-valued meals.
- Prep: a prep session shows on the day of its `weekday`. Sessions without a weekday or without tasks are not shown, and the week's prep introduction shows only above at least one shown session. There is no empty prep message.
- Database failure: HTTP 503 and a generic availability message. Never present a database failure as a missing plan; log details on the server.

The HTML response itself has the correct status; do not serve HTTP 200 for every path and defer missing-plan detection entirely to the browser.

## Interface preservation

The [design system](design-system.md) is the visual reference. Follow its tokens, type and components: one centred main column of at most 560px on every width, and a light and a dark theme. The [example page](examples/menu.html) is kept as history.

Retain:

- Expandable meal, prep, and shopping cards.
- Meal descriptions, nutrition values, ordered instructions, and tip callouts.
- Daily nutrition totals.
- Shopping-list categories and notes; multiple lists can appear on a day in stored order.

Intentional changes:

- The interface is redesigned after Kandinsky's Bauhaus years: a cream canvas, thin black lines, Jost and mono type, and a primary per job, each with its shape (a blue circle for Sous, a red square for shopping, a yellow triangle for prep). It has no emoji in its own text (household data is rendered as given).
- Every page ends with a theme toggle: Light, Dark or System. System is the default and follows the device's color-scheme preference.
- Shopping items become plain lists: rows separated by hairlines, with no bullets. Remove checkbox controls, completion counts, checked colors, and strikethrough state.
- Display days in ISO week order.
- Add year and missing-plan pages using the design system's typography and column.
- Week, date, missing-plan and invalid pages show a month calendar. On week, date and missing-plan pages it is hidden until the day heading opens it, right below the heading; on invalid pages it is always shown, below the message. Every date is a link to its date path, week rows show the week number and a circle for planned weeks, and a day with a shopping list or prep session carries a mark (a square for shopping, a triangle for prep).
- The Daily Plan and Prep Sessions tabs and the day selector are replaced by the day view: a row with previous and next day links around the day's heading (for example “Wednesday, November 11”), then the day's nutrition summary and meals, then its shopping lists, prep sessions and reminder. The missing-plan page has the same row for its date. Day and missing-plan pages show a "Today" link in the header row, to today's date path, when the displayed date isn't today.
- The reference page's Strategy tab is removed.

Move all week-specific text into the database, including the header subtitle, prep introduction, and optional day reminder. Keep “Meal Plan”, weekday and month names, empty-state messages, and nutrition labels as UI text.

Continue using the existing React component structure. Compile JSX at build time instead of loading Babel in the browser. Bundle React and self-host the reference fonts so page rendering does not depend on public CDNs. The interface uses two self-hosted families: Jost for words and JetBrains Mono for numbers.

Expanded cards and the open calendar are transient browser state. The theme choice is the only UI state kept, in the browser's local storage. No shopping progress is required. Text from the database is rendered as escaped text, never executable HTML. Preserve keyboard access to day links, calendar dates and expandable controls.

## Database contract

Use relational tables with stable UUID text IDs for independently addressable content. Keep IDs stable when editing or reordering existing records. Week identity is the unique `(iso_year, iso_week)` pair.

Ordered child collections have an integer `position >= 0`, unique within their parent. Days instead use their unique weekday number. Positions need not be contiguous. Queries always order explicitly. Required text columns are non-null; optional text uses NULL. No stored HTML, JavaScript, or arbitrary CSS.

| Table | Fields and constraints |
| --- | --- |
| SQLx migration bookkeeping | Applied migration metadata; managed by SQLx only, not part of the content contract. |
| `weeks` | `id` PK, `iso_year`, `iso_week`, optional `subtitle`, optional `prep_intro`; unique year/week; application/writer validates ISO week existence. |
| `days` | `id` PK, `week_id` FK, `weekday` integer 1–7 (Monday=1), optional `reminder_title`, optional `reminder_body`, optional `reminder_icon`; unique week/weekday. |
| `meals` | `id` PK, `day_id` FK, `position`, `type` enum Breakfast/Lunch/Dinner/Snack, `name`, `description`, `kcal`, `protein_g`, `fat_g`, `carbs_g`, `fiber_g`, `for_two` boolean, optional `assembly_time`, optional `cook_time`, optional `tip`. |
| `meal_steps` | `id` PK, `meal_id` FK, `position`, `text`. |
| `shopping_lists` | `id` PK, `week_id` FK, `position`, `label`, optional `note`. |
| `day_shopping_lists` | `day_id` FK, `shopping_list_id` FK, `position`; composite PK day/list; unique day/position. Both records must belong to the same week. |
| `shopping_categories` | `id` PK, `shopping_list_id` FK, `position`, `name`. |
| `shopping_items` | `id` PK, `category_id` FK, `position`, `text`. |
| `prep_sessions` | `id` PK, `week_id` FK, `position`, `title`, optional `covers`, optional `weekday` integer 1–7 (the day it shows on). |
| `prep_tasks` | `id` PK, `session_id` FK, `position`, `name`, `time`, `instructions`. |

Parent deletion cascades to owned children and association records. Index foreign keys used to assemble a week. Enforce cross-week shopping associations with insert/update triggers, including attempts to move referenced records between weeks.

Week date ranges and day dates derive from ISO year/week and weekday; do not store duplicate date labels. A week may omit day records; the response still contains seven ordered days with empty content for missing records.

Nutrition values are required non-negative numbers. Values represent the displayed individual's daily intake, matching the sample's summation behavior. `for_two` controls only the FOR TWO badge; never multiply nutrition by two. Mixed-person meal descriptions remain free text. Daily totals are derived from stored meal values, not stored separately. Sum before formatting and display at most one decimal place, dropping trailing `.0`.

Times remain display text, such as “35 min (mostly air fryer + simmer)”. Shopping quantities remain part of item text. Do not parse, consolidate, or reinterpret them.

### Data consistency

The schema is a documented integration contract; implementing data entry is outside scope. IDs remain stable across edits. Related changes must commit atomically, so readers see either the previous or committed state, never a partially written plan. Foreign-key and semantic constraints, including valid ISO weeks and supported types, apply to all stored content.

## Read API and rendering

The page handler reads a complete week in one read transaction and embeds its initial JSON document safely into the HTML shell. This avoids a second initial data request and ensures status, headings, dates, and content describe the same snapshot. Escape embedded JSON appropriately so text such as `</script>` cannot terminate its container.

The week, date, missing-plan and invalid pages also embed `today` (the server's today in `APP_TIMEZONE`), `plannedWeeks` (every stored week, ascending by ISO week-year and week), `displayedDate` (the shown day, or null) and `markedDays` (`{ date, shopping, prep }` for every date of every stored week that has a shopping list or a shown prep session, ascending). On week and missing-plan pages these are read in the same read transaction as the week. The JSON API returns the week document only.

The JSON endpoint exposes the same document shape:

```text
schemaVersion
week: { id, isoYear, isoWeek, startDate, endDate, subtitle, prepIntro }
days: [ { id|null, date, weekday, meals: [...], shoppingListIds: [...], reminder|null } ]
shoppingLists: [ { id, label, note, categories: [ { id, name, items: [...] } ] } ]
prepSessions: [ { id, title, covers, weekday|null, tasks: [...] } ]
```

Nested meals and tasks carry their stored display fields and IDs. Arrays are ordered. Nullable fields are explicit; absent collections are empty arrays. The API document does not expose internal migration tables or filesystem paths. Missing/invalid weeks return 404 with a stable JSON error code; database failures return 503.

`/api/<year>` returns the year page's data (`year`, `previousYear`, `nextYear`, `weeks`) from one query, with the year page's status rule: a year without plans is 404 `year_not_found` unless it is the current year. Any two-segment path under `/api` is a week path: a valid week returns its document, and an invalid year or week is `invalid_week`. A single segment is a year, a valid date (the document of its week), a date-shaped but impossible or unrepresentable date (`invalid_date`), or otherwise `not_found`. Any path under `/api` that matches no route returns `not_found`.

Do not cache HTML or week JSON: send `Cache-Control: no-store`. Serve content-hashed static assets with long-lived immutable caching. No application data cache or restart is needed to observe committed writes. An open page changes only after navigation or refresh.

Request handlers use read-only database connections. Mutation requests to defined page and API routes receive 405. Do not enable broad cross-origin access.

## Verification and acceptance criteria

1. A populated fixture renders each day's cards, text, badges, and instructions styled as the design system specifies, in light and dark. Compare desktop and narrow mobile screenshots, including expanded states; documented shopping/day-order/archive changes are expected.
2. `/` resolves the current ISO week in the configured timezone; `/2026/44` shows October 26–November 1, 2026.
3. Test ISO year boundaries, valid/invalid week 53, timezone midnight boundaries, canonical redirects, date paths, and the day shown for old, current and future weeks.
4. Missing current plans, missing explicit weeks, empty archives, missing days, and unavailable databases produce their specified messages and HTTP statuses.
5. Editing a meal through a separate database connection becomes visible on refresh without restarting. An uncommitted transaction is never partially rendered.
6. Shopping lists render categorized plain rows with no checkboxes or progress state. Collection ordering remains stable after edits.
7. Nutrition totals sum stored values once per meal regardless of FOR TWO, and decimal formatting is deterministic.
8. Database constraints reject invalid references, duplicate sibling positions, and cross-week shopping associations.
9. User-supplied text is escaped in both React rendering and initial JSON embedding.
10. Pages render without third-party CDN access.

The sample’s Sunday–Saturday date range spans two ISO weeks. Use an explicitly dated fixture for visual verification rather than silently shifting its dates.
