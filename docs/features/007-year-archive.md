# 7. Year archive pages

## Outcome

The archive is served one year at a time at `/<year>`, replacing the single all-years list at `/weeks`. Each year page links to the previous and next years when they have plans.

Depends on: [2. Calendar navigation and archive](002-calendar-navigation.md). This feature replaces the `/weeks` route defined there.

## Scope

- Add `/<year>`, listing the stored plans of that ISO week-year in descending week order with date ranges and permanent links, as `/weeks` does today. Include future plans. A week belongs to the year in its URL: `/2026/1` (December 29, 2025–January 4, 2026) is listed on `/2026`.
- A year path is exactly four ASCII digits from 0001 to 9999, the same form as the year in week URLs. Any other single-segment path, such as `/nothing`, `/26`, `/0000` or `/02026`, keeps the plain not-found response.
- Remove `/weeks`. It returns the plain not-found response.
- Page heading: “Meal Plan: 2026”. Document title: “2026 — Meal Plan”. Week entries are unchanged.
- Resolve the current year as the ISO week-year of today in `APP_TIMEZONE` on every request, the same way `/` resolves the current week.
- Update the README route table and the technical design to match.

## Navigation links

| Page | Link | Target |
| --- | --- | --- |
| Week plan and missing-plan pages, at `/` or `/<year>/<week>` | “All weeks” | `/<year>` of the displayed week's ISO week-year |
| Invalid week page | “All weeks” | `/<current year>` |
| Year page | “This week” | `/` |
| Year page | Previous year, e.g. “← 2025” | `/<year − 1>`, only when that year has at least one stored plan |
| Year page | Next year, e.g. “2027 →” | `/<year + 1>`, only when that year has at least one stored plan |

Year links point to the adjacent year only. When the adjacent year has no plans, omit the link, even if an earlier or later year has plans. Year links appear on empty year pages too.

The previous-year, “This week” and next-year links appear in that order in the header row, where “This week” is now, with the same style. “All weeks” keeps its current place and style.

## Missing and invalid years

| Situation | HTTP status | Presentation |
| --- | --- | --- |
| Current year has no plans, including an empty database | 200 | Heading, “No meal plans available yet.”, and the year page links. |
| Any other valid year has no plans | 404 | Same page. |
| Path is not a year, or `/weeks` | 404 | Plain not-found response. |
| Database unavailable | 503 | Generic availability message, as on other pages. |

Return the status in the initial HTML response, not only in browser state.

## Acceptance criteria

1. `/2026` lists exactly the stored weeks of ISO week-year 2026 in descending order, including future weeks. `/2026/1` is listed on `/2026` and `/2020/53` on `/2020`.
2. “All weeks” on `/2025/50`, with or without a stored plan, links to `/2025`. At `/` it links to the current ISO week-year, including on December 29, 2025, when it links to `/2026`. On the invalid week page it links to the current year.
3. Previous and next year links appear only when the adjacent year has at least one stored plan. They never link to a year without plans.
4. Empty-year statuses and messages match the table above. With an empty database, the “All weeks” link from `/` leads to a 200 page.
5. `/weeks`, `/weeks/` and non-year single-segment paths return the plain 404 without server errors.
6. Year pages and their year links reflect committed database changes on refresh.

## Verification

Use table-driven HTTP tests with a fixed clock, covering ISO year boundaries, the current-year rule around New Year in the configured timezone, gap years, the first and last supported years, empty databases and database failure. In the browser, verify the “All weeks”, “This week” and year links navigate correctly, and capture year pages at 375 px and 1280 px. Update the [verification record](../verification.md).
