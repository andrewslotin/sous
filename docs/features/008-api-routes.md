# 8. API routes matching page routes

## Outcome

API paths mirror the page paths with an `/api` prefix: the week document moves from `/api/weeks/<year>/<week>` to `/api/<year>/<week>`, and `/api/<year>` returns the year listing shown on the year page. No API path contains `/weeks`.

Depends on: [7. Year archive pages](007-year-archive.md). This feature replaces the `/api/weeks/<year>/<week>` route defined in [5. Read API and service verification](005-read-api.md).

## Scope

- Move the week document to `GET /api/<year>/<week>`. Its body, statuses, error codes, ISO validation and zero-padded redirects are unchanged, except that redirects target `/api/<year>/<week>`. As with page routes, any two-segment path under `/api` is a week path: `/api/abc/1` and `/api/weeks/2026` are `invalid_week`.
- Remove `/api/weeks/<year>/<week>`. It returns the JSON `not_found` response.
- Add `GET /api/<year>`. A year path follows the year page rule: exactly four ASCII digits from 0001 to 9999. Any other single segment, including `weeks`, returns the JSON `not_found` response.
- The year response contains the same weeks and the same adjacent-year rule as the year page, computed from one database snapshot. The current year is the ISO week-year of today in `APP_TIMEZONE`.
- Leave the document `schemaVersion` at `1`; the week document format does not change.
- Update the [OpenAPI document](../openapi.json), the README route table, the technical design and the MCP guide, which cite the old week path.

## Year response

```json
{
  "schemaVersion": 1,
  "year": 2026,
  "previousYear": 2025,
  "nextYear": null,
  "weeks": [
    {"isoYear": 2026, "isoWeek": 44, "startDate": "2026-10-26", "endDate": "2026-11-01", "path": "/2026/44"}
  ]
}
```

`weeks` uses the existing week summary in descending week order, including future weeks; `path` stays the canonical page path. `previousYear` and `nextYear` are the adjacent years when they have at least one stored plan, otherwise null.

## HTTP contract for `/api/<year>`

| Situation | Status | JSON body |
| --- | --- | --- |
| Year with plans | 200 | Year response. |
| Current year without plans, including an empty database | 200 | Year response with an empty `weeks` array. |
| Any other valid year without plans | 404 | `{"error":{"code":"year_not_found","message":"No meal plans available yet."}}` |
| Not a year | 404 | `{"error":{"code":"not_found","message":"Not found."}}` |
| Database unavailable | 503 | `{"error":{"code":"service_unavailable","message":"Meal plans are temporarily unavailable."}}` |

Use `application/json` and `Cache-Control: no-store`, as for the week document. Mutation methods on both routes return 405.

## Acceptance criteria

1. `/api/2026/44` returns the same document that `/api/weeks/2026/44` returns today, and `/api/2026/044` redirects to `/api/2026/44`.
2. `/api/weeks/2026/44`, `/api/weeks`, `/api/0000`, `/api/10000` and `/api/nothing` return the JSON `not_found` response.
3. `/api/2026` lists the same weeks in the same order as the `/2026` page, and its `previousYear` and `nextYear` match the page's year links, including gap years.
4. All `/api/<year>` cases above have the specified status, content type and stable JSON code.
5. The OpenAPI document describes both routes and their error codes, and contains no `/weeks` path; a test validates the example responses against it.
6. The API reflects committed database changes on the next request.

## Verification

Extend the API contract tests with a fixed clock, covering ISO year boundaries, the current-year rule around New Year, gap years, empty databases and database failure. Compare each year response with the bootstrap data of the matching year page.
