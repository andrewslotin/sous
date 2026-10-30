# 5. Read API and service verification

## Outcome

Expose the complete week document as a read-only JSON API and verify the finished service against the technical design. Earlier features retain responsibility for their own tests.

Depends on: [4. Prep, strategy, and reminders](004-weekly-content.md).

## Scope

- Add `GET /api/weeks/<year>/<week>` using the same application query and document types as the HTML page.
- Set document `schemaVersion` to `1`; this is the API format version, independent of SQL migration versions.
- Serialize field names in camelCase, stable UUID IDs as strings, dates as `YYYY-MM-DD`, nullable fields explicitly as null, and absent collections as empty arrays.
- Return seven ordered day entries, with null IDs for missing day records, and all nested content described in the [read contract](../technical-design.md#read-api-and-rendering).
- Document the complete response fields and provide an example response derived from the fixture when implementing the endpoint.
- Verify the complete page and API share one coherent database snapshot per request.

## HTTP contract

| Situation | Status | JSON body |
| --- | --- | --- |
| Existing week | 200 | Complete week document. |
| Valid week without a plan | 404 | `{"error":{"code":"week_not_found","message":"No meal plan for this week yet."}}` |
| Invalid year/week | 404 | `{"error":{"code":"invalid_week","message":"Invalid week."}}` |
| Database unavailable | 503 | `{"error":{"code":"service_unavailable","message":"Meal plans are temporarily unavailable."}}` |

Use `application/json` for JSON responses, including errors. Apply the same ISO validation as page routes; redirect valid zero-padded week paths to their canonical API path. Do not include database paths, SQL, or migration internals in responses.

Dynamic HTML and JSON responses use `Cache-Control: no-store`. Content-hashed static assets use immutable caching. Mutation methods on defined page/API routes return 405. Do not enable broad cross-origin access. No write endpoints are added.

## Acceptance criteria

1. HTML bootstrap data and API JSON describe the same fixture week, including ordering, dates, IDs, and null/empty fields.
2. All error cases have the specified HTTP status, content type, and stable JSON code.
3. A concurrent external transaction cannot produce a response combining old and new related records. Test this with controlled synchronization, not timing-dependent sleeps.
4. The next request after commit observes updated data, without application restart or stale caching.
5. Mutation methods return 405 and database error details do not leak into responses.
6. The built binary serves its complete interface with no third-party asset requests. Missing asset paths return 404 rather than a plan page.
7. Mobile/desktop visual verification covers all tabs, expanded cards, empty states, and archived/current week selection. Differences are limited to those approved in the technical design.
8. Every acceptance criterion in the technical design is covered by an automated test or a recorded manual verification result.

## Verification

Add API contract and controlled-concurrency tests, then run the combined Rust and frontend checks from all iterations. Record the final visual checks and resolve regressions before marking the feature Done. Do not add deployment, backup, write-tool, or MCP work to this iteration.
