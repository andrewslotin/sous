# 11. Calendar sidebar

## Outcome

Week pages show a month calendar on the side. Picking a week in it opens that week's page. The calendar takes over from the "All weeks" link as the way to move between weeks. Year pages stay, but no page links to them anymore.

Depends on: [2. Calendar navigation and archive](002-calendar-navigation.md) and [7. Year archive pages](007-year-archive.md). This feature changes the "All weeks" rows in feature 7's "Navigation links" table and the "All weeks" link added in feature 2.

## Scope

- Show the calendar on the week plan page, the missing-plan page (at `/` and at `/<year>/<week>`) and the invalid week page. Year pages and the unavailable (503) page do not show it.
- Remove the "All weeks" link from every page that has it: the plan page, the missing-plan page and the invalid week page. The invalid week page keeps its "This week" link.
- Keep `/<year>` and `/api/<year>` exactly as they are. Year pages keep their "This week" and year links.
- HTTP statuses, headings, page text and day selection on all pages stay unchanged.

## Calendar

The calendar shows one month at a time:

```text
‹   November 2026   ›
Wk  Mo Tu We Th Fr Sa Su
44                    1
45   2  3  4  5  6  7  8
46   9 10 11 12 13 14 15   ← displayed week
47  16 17 18 19 20 21 22
48  23 24 25 26 27 28 29
49  30
```

- The heading is the month name and calendar year, e.g. "November 2026". Days run Monday to Sunday. The first column is the ISO week number, headed "Wk".
- There is one row for each ISO week that contains at least one day of the month, so a month has four to six rows. A row shows all seven of its dates. Dates outside the month are dimmed.
- Each row is a single link to the week's canonical path `/<isoYear>/<isoWeek>`. Every week can be picked, planned or not. A week without a plan opens the existing missing-plan page. The link's accessible name gives the week number, its date range and whether it is planned, e.g. "Week 46, November 9–15, 2026, planned".
- Rows of weeks with a stored plan carry a visible "planned" mark. A plan is stored when its `weeks` record exists, as defined in the technical design.
- The displayed week's row is highlighted and has `aria-current="page"`. Today's date is marked. "Today" is the server's today in `APP_TIMEZONE`, never the browser's date.
- ‹ and › buttons, labelled "Previous month" and "Next month", change the month in the browser. They do not fetch data, change the URL or change the displayed week.

### Initial month

| Page | Month shown on load |
| --- | --- |
| Week plan or missing-plan page | The month that contains the displayed week's Thursday. For example, `/2026/44` (October 26–November 1) opens on October 2026. |
| Invalid week page | The month that contains today. |

### Range limits

The calendar does not go past January 0001 or December 9999. The ‹ button is disabled on January 0001 and the › button on December 9999. A row whose seven-day range cannot be represented, such as the last row of December 9999, is shown but is not a link. This matches the week path rule in feature 2.

## Layout

| Viewport width | Presentation |
| --- | --- |
| 900 px and wider | The calendar is a sidebar to the left of the 540 px content column. The content column keeps its width, and the two are centred as a group. There is no toggle button. |
| Narrower than 900 px | The calendar is hidden. A "Calendar" button in the header row opens it between the header and the page content. Pressing it again closes it. The button has `aria-expanded` and controls the calendar. The calendar is closed on every page load. |

The "Calendar" button sits where "All weeks" was, with the same style, so the header row stays the same on every page kind. The calendar uses the existing fonts, colors, borders and card style. The selected-week highlight uses the same gradient as the selected day and tab buttons.

## Data

- The embedded page data of the week plan, missing-plan and invalid week pages gains two fields next to `kind`:
  - `today`: the server's today in `APP_TIMEZONE`, as an ISO date.
  - `plannedWeeks`: every stored week as `{ "isoYear", "isoWeek" }`, in ascending order.
- On the week plan and missing-plan pages, read `plannedWeeks` in the same read transaction as the week, so the calendar and the page describe the same snapshot.
- The week `document` keeps its current shape, so the embedded document and `/api/<year>/<week>` still match. The JSON API does not change.

## Documentation

- Technical design: replace the "All weeks" link in the missing-data rules and in "Intentional changes" with the calendar. Note that the calendar sidebar sits outside the 540 px content column. State that year pages are still served but not linked.
- README: the route table stays as it is. Update any text that mentions the "All weeks" link.

## Acceptance criteria

1. The week plan, missing-plan and invalid week pages show the calendar and have no "All weeks" link. Year pages and the 503 page are unchanged.
2. `/2026/44` opens the calendar on October 2026. `/2026/1` (December 29, 2025–January 4, 2026) opens on January 2026. The invalid week page opens on today's month.
3. Every linked row goes to the canonical path of its week, including weeks across ISO year boundaries. For example, the row with December 29, 2025 links to `/2026/1`, and the row with January 1, 2021 links to `/2020/53`.
4. Exactly the weeks with stored plans carry the planned mark, and the accessible names say so. The displayed week's row has `aria-current="page"`. Today's mark follows the server's clock in `APP_TIMEZONE`, not the browser's timezone.
5. Changing the month sends no request and leaves the URL and displayed week unchanged. Returning to the initial month shows the same highlight.
6. At January 0001 and December 9999 the matching month button is disabled. Unrepresentable rows are not links.
7. At 1280 px the calendar is a sidebar beside a 540 px content column, with no "Calendar" button. At 375 px the calendar is hidden on load, and the "Calendar" button opens and closes it with the correct `aria-expanded` value.
8. `plannedWeeks` reflects committed database changes on refresh. The embedded week document still equals the `/api/<year>/<week>` response.
9. `/<year>` and `/api/<year>` behave as feature 7 specifies.

## Verification

Add HTTP tests with a fixed clock and temporary databases for the new page data fields: `today` around midnight in the configured timezone, `plannedWeeks` order and content, the empty database, and that the embedded document still equals the API response. Extend the browser tests (`cargo test --test browser -- --ignored`) to cover the initial month, row links across year boundaries, the planned mark and `aria-current`, month navigation without requests, the range limits and the toggle. Capture the week plan page with the calendar at 375 px (closed and open) and at 1280 px.
