# 16. Today link

## Outcome

From any other day, one tap brings the reader back to today. Day pages and the missing-plan page show a "Today" link in the header row whenever the displayed date isn't today.

Depends on: [14. Redesign](014-redesign.md).

## Scope

- Add the link to day pages (`/`, `/<year>/<week>`, `/<date>`) and to the missing-plan page.
- Invalid week, invalid date, year and unavailable pages don't change. They already link to "This week" or "Try again".
- The page data already carries `today` and `displayedDate`, so this is a frontend change. Routes, statuses, data, the API and the MCP tools don't change.

## Behaviour

- The link is shown when `displayedDate` isn't `today`, and isn't rendered when they are equal.
- It points to today's date path, e.g. `/2026-11-16`, not to `/`. Following it loads that page, with the calendar closed, as any page load does.
- Today is the server's `today`, resolved in `APP_TIMEZONE`, the same date as the calendar's today mark. The browser's clock is never used. A page left open past midnight keeps its own `today` until it is reloaded.
- If today's week has no plan, the link leads to the missing-plan page for today.
- Examples, with today Monday, November 16, 2026:

| Page | Displayed date | Link |
| --- | --- | --- |
| `/` | Nov 16 | None |
| `/2026/47` | Nov 16 | None |
| `/2026-11-18` | Nov 18 | `/2026-11-16` |
| `/2026/44` | Oct 26 | `/2026-11-16` |
| `/2026/50`, no plan | Dec 7 | `/2026-11-16` |

## Layout and styles

- The link is the last item in the header's `.header-meta` row, after the subtitle, and sits at the right end of the row, like the year page's "This week" link.
- With no subtitle, the row holds only the link, on the right. Without the link and without a subtitle, the row isn't rendered, as now.
- The link uses the existing header-link style: `small` `ink-muted`, underlined in `line-strong`, turning `ink` on hover. It gets the shared focus outline.
- Its text and accessible name are "Today". It has no icon or emoji.
- A long subtitle wraps beside the link. Nothing overflows at 320 px.

## Markup

- `<a class="all-weeks today-link" href="/YYYY-MM-DD">Today</a>` inside `.header-meta`, rendered by the shared day-page shell, so day pages and the missing-plan page get it from one place.

## Documentation

- Design system, Components, Header: replace "Day pages have no links" with "Day and missing-plan pages show `Today` when the displayed date isn't today".
- Technical design, "Intentional changes": add the Today link to the day view's description.
- README, "Pages and API": mention the Today link.

## Acceptance criteria

1. On a day page or the missing-plan page whose displayed date isn't today, the header row ends with a "Today" link to today's date path, with today resolved by the server.
2. When the displayed date is today, there is no Today link, and the header is unchanged.
3. Invalid week, invalid date, year and unavailable pages are unchanged.
4. The link matches the design system's header links in light and in dark. It sits at the right end of the row, also when there is no subtitle. It meets 4.5:1 contrast, shows the focus outline under keyboard focus, and nothing overflows at 320 px with a long subtitle.
5. Routes, statuses, data and API responses are unchanged. The existing tests pass.

## Verification

- Browser tests, with the harness's fixed today:
  - The link appears on another day, on another week and on a missing-plan page for another week, each with today's date path. Following it opens today's date page.
  - There's no link on `/`, on the current week's path or on today's date path.
  - Invalid and year pages have no `.today-link`.
  - The link is right-aligned with and without a subtitle. A long subtitle at 320 px causes no overflow. Contrast and the focus outline hold in both themes.
- Screenshots at 375 px in light and dark: a day other than today, and today.
