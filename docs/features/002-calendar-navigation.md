# 2. Calendar navigation and archive

## Outcome

The homepage shows the current calendar week, while permanent URLs and an archive provide access to other weeks, including useful pages when data is missing.

Depends on: [1. Database-backed daily plan](001-database-backed-daily-plan.md).

## Scope

- Add `/`, resolving the current ISO week on every request using `APP_TIMEZONE`, default `Europe/Berlin`. Keep `/` in the address bar.
- Complete `/<year>/<week>` validation and canonicalization. ISO weeks run Monday–Sunday; the URL uses the ISO week-year.
- Add `/weeks`, listing stored plans in descending ISO year/week order with date ranges and permanent links. Include future plans.
- Add a discreet “All weeks” link to the plan page.
- Select today when viewing the current week at either route; select Monday for any other week. Browser timezone must not override server selection.
- Make the clock injectable for tests. Reject invalid timezone configuration instead of silently falling back.

## Missing and invalid weeks

| Situation | HTTP status | Presentation |
| --- | --- | --- |
| Current week has no plan at `/` | 200 | Date range, “No meal plan for this week yet.”, available-week links. |
| Valid explicit week has no plan | 404 | Same useful missing-plan page. |
| Invalid week/year | 404 | “Invalid week.” and current-week/archive links. |
| Archive has no entries | 200 | “No meal plans available yet.” |
| Database unavailable | 503 | Generic availability message, not a missing-plan page. |

Accept four-digit years 0001–9999 only if the full seven-day range is representable. Validate week 53 against its year. Redirect valid zero-padded week paths to the unpadded canonical path. Return the status in the initial HTML response, not only in browser state.

## Acceptance criteria

1. `/2026/44` displays October 26–November 1, 2026.
2. ISO year transitions work, including dates whose ISO year differs from their calendar year.
3. Current-week resolution and selected day are correct around midnight in the configured timezone, independent of browser timezone.
4. Past and future plan URLs remain stable and initially select Monday.
5. All missing/error cases above have the specified status, text, and useful navigation.
6. The archive orders weeks correctly across year boundaries and reflects committed database changes on refresh.
7. Valid zero-padded paths redirect; impossible week numbers and unrepresentable ranges return 404 without server errors.

## Verification

Use table-driven calendar and HTTP tests with a fixed clock, including valid and invalid week 53, year boundaries, timezone transitions, and empty databases. Verify archive links and day selection in the browser.
