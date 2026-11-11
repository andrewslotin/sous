# 13. Day view

## Outcome

The plan page shows one day at a time. The landing page shows today, and picking a date in the calendar opens that date. A day shows its shopping lists, its prep sessions and its meals together, so the Daily Plan and Prep Sessions tabs and the Monday–Sunday day bar are gone. The calendar marks the dates that have shopping or prep.

Depends on: [11. Calendar sidebar](011-calendar-sidebar.md) and [12. Remove strategy cards](012-drop-strategy.md). This feature changes feature 11's calendar from week links to date links. It replaces the day selector from feature 2 and the Prep Sessions tab from feature 4, and gives prep sessions a day.

## Scope

- Remove the tab bar and the day bar. The plan page shows one day, chosen by the route.
- Add the date page `/<date>` and the API route `/api/<date>`.
- Give each prep session an optional weekday, and show prep sessions on their day.
- Make every calendar date a link, and mark dates that have shopping or prep.
- Add previous-day and next-day links.
- Year pages and the unavailable (503) page do not change. Statuses, headings and messages stay unchanged except where this spec says otherwise.

## Routes

| Route | Displayed date |
| --- | --- |
| `/` | Today, resolved on every request in `APP_TIMEZONE`. `/` stays in the address bar. |
| `/<year>/<week>` | Today if the week is the current week, otherwise its Monday. The URL does not change. |
| `/<date>` | The date in the path, e.g. `/2026-11-11`. |
| `/api/<date>` | Not a page. Returns the document of the week that contains the date, the same body as `/api/<isoYear>/<isoWeek>`. |

The displayed date is always set by the server; the browser's timezone never changes it.

A date path is a single segment of the form `YYYY-MM-DD` in ASCII digits. It is valid when it is a real Gregorian date with a year from 0001 to 9999 and its whole ISO week can be represented, which is the same rule that feature 2 sets for week paths. There are no redirects: `/2026-11-11` and `/2026/46` are both canonical. A single segment that is neither a year nor shaped like a date is still a plain 404 page, or `not_found` under `/api`.

| Situation | Status | Page | `/api/<date>` |
| --- | --- | --- | --- |
| Valid date in a week with a plan | 200 | Day view | Week document |
| Valid date in a week without a plan | 404 | The missing-plan page for the date's week | `week_not_found` |
| Date-shaped but invalid, e.g. `/2026-02-30`, `/0000-01-01`, `/9999-12-31` | 404 | "Invalid date." with the "This week" link and the calendar | `invalid_date` |

The missing plan at `/` is still HTTP 200.

## Day view

From top to bottom:

1. The header row, unchanged: title, subtitle and, on narrow viewports, the "Calendar" button.
2. The day row, where the tabs and day bar used to be. It holds a "Previous day" link (‹), the day heading, e.g. "Wednesday, November 11", and a "Next day" link (›).
3. The shopping list cards assigned to the day, in the day's order, unchanged.
4. Prep: the week's prep introduction, followed by the cards for the day's prep sessions in session order. The introduction appears only on days that show at least one prep session. Prep cards keep their appearance, expansion and task fields.
5. The nutrition summary and meal cards, or "No meals planned for this day.", unchanged.
6. The day's reminder, unchanged.

The ‹ and › links go to the date path of the previous and next date, also across week and year boundaries. Following one loads that page. A link whose target is not a valid date path is shown disabled and is not a link. That happens on 0001-01-01 for ‹ and on 9999-12-26 for ›.

A prep session is shown on the day matching its weekday. A session without a weekday appears on no day. As before, a session without tasks is not shown. The message "No prep sessions planned." is removed.

The missing-plan and invalid date pages have no day row. Expanded cards remain transient browser state.

## Prep session day

- Add migration `0005`, which adds a nullable `prep_sessions.weekday` integer that must be 1–7 (Monday=1). Existing sessions get NULL, so they are not shown until a save gives them a weekday. All other content stays.
- The session stores a weekday of its own week, not a reference to a day record. A session therefore needs no day record and cannot point into another week.
- In the week document, each item of `prepSessions` gains `weekday` (1–7 or `null`) after `covers`. This covers the embedded page data, `/api/<year>/<week>`, `/api/<date>` and `get_week`. Set `schemaVersion` to `3`.
- In `save_week`, `weekday` is required on every prep session and may be `null`. A missing key, or any value other than 1–7 or `null`, is `invalid_input` and changes nothing.
- The week revision covers the new column, so a change to only a session's weekday changes the revision.
- Development fixtures give their prep sessions weekdays: in 2026-W44, the Monday session gets 1 and the Wednesday session gets 3.

## Calendar

This section changes feature 11's calendar:

- Every date is a link to its date path, including the dimmed dates outside the month. A date whose path is not valid is not a link. This replaces feature 11's rule for unrepresentable rows.
- Week rows are no longer links. The "Wk" column and the planned mark stay on the row as plain text.
- The displayed date gets the selected highlight and `aria-current="page"`, in place of the displayed week's row highlight. Today's mark stays.
- A date has a shopping mark when its day view shows at least one shopping list, and a prep mark when it shows at least one prep session. The marks follow the day view rules, so a session without a weekday or without tasks adds no mark. The two marks look different from each other and from today's mark, and use the existing shopping and prep card colors. A date can have both.
- A date link's accessible name gives the full date followed by its marks, e.g. "Wednesday, November 11, 2026, shopping, prep".
- At `/` and `/<year>/<week>`, the calendar opens on the month that feature 11 specifies, the month of the week's Thursday, so `/2026/1` still opens on January 2026. At `/<date>`, including its missing-plan page, it opens on the month of that date. On the invalid date page it opens on today's month, like the invalid week page.
- Month navigation, range limits on the month buttons and the layout stay as in feature 11.

## Data

- The embedded page data gains two fields next to feature 11's fields:
  - `displayedDate`: the displayed date as an ISO date, or `null` on the invalid date and invalid week pages.
  - `markedDays`: every date of every stored week that has a mark, as `{ "date", "shopping", "prep" }` with boolean marks, in ascending date order.
- Read `markedDays` in the same read transaction as the week and `plannedWeeks`.
- Apart from `weekday` and `schemaVersion`, the week document does not change. The embedded document still equals the API response.

## Documentation

- Technical design: add `/<date>` and `/api/<date>`, the date path rule and `invalid_date`. Replace the day selection paragraph with the displayed date rule. Under "Retain", remove the tabs and the day selector. Under "Intentional changes", say that the day view and the calendar replace the tabs and the day bar. Remove the prep tab from the empty-state rule and "tab names" from the UI text. Add `weekday` to `prep_sessions` and to the document shape.
- OpenAPI document: add `/api/{date}` and `invalid_date`, add `weekday` to the prep session schema and the example, and set `schemaVersion` to `3`.
- MCP guide: add the `weekday` rule to the save rules and the example call. Update any tool description in the server that describes prep sessions, and regenerate the tool listing from `tools/list`.
- Save example: add `weekday` to the prep session.
- README: add `/<date>` and `/api/<date>` to the route table, and update any text about tabs or the day bar.
- Features 1–12 stay as history.

## Acceptance criteria

1. `/` shows today's day view, with no tabs and no day bar, correctly around midnight in `APP_TIMEZONE` and independent of the browser's timezone.
2. `/2026-11-11` shows Wednesday, November 11, 2026. `/2026/46` shows today while it is the current week, and Monday otherwise. `/2025-12-29` shows the Monday of 2026-W1, and `/api/2025-12-29` equals `/api/2026/1`.
3. Every situation in the status table gives its status, page and error code. `/2026-1-5` and `/2026-11` are plain 404s, and `not_found` under `/api`.
4. The day view shows its parts in the specified order. The prep introduction appears only on days with a prep session. Prep cards expand with keyboard and pointer input and show each task's name, time and instructions.
5. A prep session without a weekday, or without tasks, appears on no day and adds no calendar mark.
6. ‹ and › go to the adjacent date, also across weeks and years: › on `/2026-01-04` goes to `/2026-01-05`, and ‹ on `/2025-12-29` goes to `/2025-12-28`. They are disabled on 0001-01-01 and 9999-12-26.
7. Each valid calendar date links to its date path, and the displayed date has `aria-current="page"`. The shopping and prep marks appear on exactly the dates whose day view shows shopping or prep, and the accessible names list them. `/2026/1` opens the calendar on January 2026, `/2025-12-29` on December 2025, and the invalid date page on today's month.
8. A database with prep sessions upgrades without errors. Its sessions have `weekday: null` and all other content is unchanged.
9. Documents have `schemaVersion: 3` and `weekday` on each prep session. `save_week` accepts 1–7 and `null`, and returns `invalid_input` for 0, 8 or a missing key without changing anything. Changing only a weekday changes the revision.
10. `markedDays` reflects committed database changes on refresh. The embedded week document still equals the API response.
11. The OpenAPI document, the MCP tool listing and the save example match the code.

## Verification

Add HTTP tests with a fixed clock and temporary databases for date path validity and statuses, ISO year boundaries, `/api/<date>` equality with the week route, `displayedDate` and `markedDays`. Add a migration test that applies `0001`–`0004`, stores prep sessions, runs `0005` and compares the documents apart from `weekday` and `schemaVersion`. Extend the save tests for `weekday` and the revision. Update the browser tests (`cargo test --test browser -- --ignored`) to cover the day view order, prep cards, the day links across week boundaries, the date links, marks and `aria-current`, and the initial month. Capture a day with shopping and prep at 375 px (calendar closed and open) and at 1280 px.
