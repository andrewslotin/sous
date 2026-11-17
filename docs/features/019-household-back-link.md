# 19. Back link on the household page

## Outcome

The household page's "This week" link becomes a back link that returns the user to the page they came from and names it: "Back to Wednesday, November 11". People open the household page from a day they're planning, check or fix one fact, and want that day back, not this week.

Depends on: [18. Read-first household page](018-household-read-first.md).

## Scope

This changes the footer link on every page, the household page's header and the household page's initial JSON. Routes, statuses, the profile API, the MCP tools and storage don't change. It replaces feature 18's header, which kept the "This week" link from feature 17.

## Behaviour

### Footer link

- The footer's "Household" link carries the page it's on, as a `from` query parameter:
  - On a page with the day row (a day page, a week page or a week without a plan), `from` is the displayed day's date path: `/profile?from=/2026-11-11`. A week page shows a day, so it returns to that day, not to the week.
  - On a year page, `from` is the year path: `/profile?from=/2026`.
  - On every other page (invalid week, invalid date, unavailable), the link is `/profile` with no `from`.
- The link's text, look and place don't change.

### Reading `from`

- `GET /profile` reads an optional `from` query parameter. The server decides where Back goes. It never echoes `from` back: it builds the path again from what it parsed. A bad or unknown `from` never makes the page fail. The page renders as normal with the fallback below.
- `from` is a **day** when it's `/` followed by a valid date path (the same rules as the `/<date>` route). It's a **year** when it's `/` followed by a valid year path (the same rules as the `/<year>` route).
- Anything else (missing, empty, a week path, a path on another site, `//…`, a URL, or text that isn't a page) falls back to **today**.
- A day equal to the server's today is also **today**.
- The household page's initial JSON gains `back`, one of:
  - `{ "kind": "day", "href": "/2026-11-11", "date": "2026-11-11" }`
  - `{ "kind": "year", "href": "/2026", "year": 2026 }` (the href is zero-padded like other year paths)
  - `{ "kind": "today", "href": "/2026-11-17" }`, with the server's today, the same today the day pages' "Today" link uses.

### Header

- The household page's header has no links row. Its back link sits above the title, left-aligned with it.
- The link's text names where it goes, in the words that page uses for itself:
  - a day: "Back to " followed by the day heading's format: "Back to Wednesday, November 11";
  - a year: "Back to 2026";
  - today: "Back to today".
- It has no arrow. The words say where it goes.
- It's a plain link to `back.href`, so it works without script history and survives a reload. Following it never needs the browser's back button.
- The household page still has no Footer link.

## Layout and styles

- The back link is `small` `ink-muted`, underlined in `line-strong` like the header links, and turns `ink` on hover. It sits `space-2` above the title, left-aligned, sized to its text (not stretched across the column).
- With no links row, the title's spacing to the first section is unchanged.
- The longest label, a day name such as "Back to Wednesday, September 30", fits on one line at 320 px.

## Documentation

- [Design system](../design-system.md), written by one owner:
  - **Header:** add the optional back link above the title, its look and its labels. Only the household page has one.
  - **Footer link:** it passes the page it's on as `from`, as above.
  - Profile page build order: header (the back link, then the title "Household"), the five profile sections, then the theme toggle footer without the Footer link.
- [Technical design](../technical-design.md): the `/profile` route row mentions the optional `from` and that a bad one falls back to today. Note the `back` object wherever the page JSON is described.

## Acceptance criteria

1. From a day page, the footer link is `/profile?from=/<that day>`. The household page then shows "Back to <weekday>, <month> <day>" above the title, linking to that day. When that day is today, it shows "Back to today", linking to today's date path.
2. From a week page or a week without a plan, the footer link carries the displayed day, and Back returns to that day.
3. From a year page, the footer link is `/profile?from=/<year>`, and Back reads "Back to <year>" and links to that year.
4. From an invalid or unavailable page, or with a missing, malformed or foreign `from`, the household page renders with 200 and shows "Back to today", linking to today's date path. `back.href` is always a path the server built, never the raw `from`.
5. The household page has no "This week" link and no links row. The back link has no arrow.
6. The back link follows the design system in light and in dark and fits at 320 px.
7. The existing tests pass after being updated for the new header and footer link.

## Verification

- Backend tests for the `back` object in the initial JSON:
  - a valid day, today's date, a year, and a zero-padded year;
  - no `from`, an empty `from`, a week path, an invalid date (`/2026-02-30`), `//example.com`, `https://example.com/2026-11-11`, `/2026-11-11/extra`, and a percent-encoded value;
  - in every case: status 200 and an `href` that is one of `/<date>`, `/<year>` or today's date path.
- Browser tests:
  - Footer link `href` on a day page, a week page, a missing week, a year page, and an invalid date page.
  - On the household page: the back link's text and href for day, today and year; that it comes before the `h1`; and that there's no "This week" link.
  - Following the footer link from a day page and then Back lands on the same day.
  - Screenshots at 375 and 1280 px, light and dark, of the household page's top.
