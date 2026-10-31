# 6. MCP meal planning and recall

## Outcome

An agent can use Sous through the Model Context Protocol (MCP) to plan meals, save plans and shopping lists, retrieve recipes from previous weeks, and answer what needs to be bought today. Sous stores and retrieves household data; the connected agent handles conversation, meal suggestions, and reasoning.

Depends on: [5. Read API and service verification](005-read-api.md).

This feature extends the read-only baseline in the [technical design](../technical-design.md). Existing page and JSON routes retain their contracts; MCP adds a separate interface with explicit write tools.

## User journeys

### Plan meals

The agent reads the current plan and relevant previous weeks, discusses meals with the user, and saves a complete week containing meals, recipes, shopping lists and their day assignments, prep sessions, strategy notes, and reminders. It can revise an existing week without changing unrelated weeks. Saved content appears in the web interface on the next refresh.

Meal generation and shopping-list composition happen in the agent. Sous does not call a model, calculate nutrition from ingredients, or automatically derive shopping items. The existing required nutrition fields and free-text quantity conventions still apply.

### “What was the recipe for the casserole I made 3 weeks ago?”

The agent gets the service's current date, searches the ISO week three weeks before the current week for casserole meals, and retrieves the matching meal's saved description, ordered steps, timings, tip, and nutrition. Results include the meal date and a link to its week so the agent can identify the source.

For this household, the agent may assume a past planned meal was cooked. No cooking confirmation or separate cooking log is required. If multiple meals match, return all candidates so the agent can identify them or ask which one the user meant. If nothing matches, return an empty result; do not silently broaden the date range or invent a recipe. The agent can explicitly search another range when appropriate.

Historical lookup returns the recipe stored on that dated meal. Reusing a dish in a future week creates a separate meal record; editing the future recipe must not alter the old meal. This does not add revision history: explicitly editing a past meal changes what future lookups return.

### “What's on my shopping list for today?”

The agent requests today's shopping lists without having to calculate the date. Sous resolves today in `APP_TIMEZONE` and returns only lists assigned to that date, with their labels, notes, categories, and item text in stored order. Return each assigned list once; preserve separate lists and quantities without merging or deduplicating items. Unassigned lists and lists assigned only to other dates are excluded.

Distinguish a missing weekly plan from an existing plan with no lists assigned to today. An assigned empty list retains its label and note with empty content. There is no purchased/completed state.

## MCP interface

- Serve a Streamable HTTP MCP endpoint at `/mcp` from the existing Rust binary and listener. No Node runtime, sidecar service, or direct database access is required by the client.
- Use a maintained Rust MCP implementation compatible with Axum/Tokio where practical. Document and test the supported protocol revision and client connection procedure during implementation. Follow the published [transport contract](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports) and [tools contract](https://modelcontextprotocol.io/specification/2026-07-28/server/tools); do not assume legacy session initialization applies to every revision.
- Expose discoverable tools with descriptions, strict input schemas, structured output schemas, and accurate read/write and destructive-operation annotations. Return structured results with a text representation for clients that need it.
- Reuse application queries and document types across MCP, HTML, and the JSON API. Transport handlers must not duplicate calendar or meal-plan logic.
- Each read observes one coherent database snapshot. Committed writes are visible on the next request; do not cache household content.
- Keep the existing loopback default. Validate request origins and hosts against configured allowed values, reject invalid origins, and do not enable wildcard CORS. Remote access uses a trusted, authenticated access layer; building accounts, OAuth infrastructure, and deployment automation is outside this feature.

### Read tools

| Tool | Inputs | Result |
| --- | --- | --- |
| `get_context` | None. | Server-local `today`, timezone, current ISO year/week, and current week's start/end dates. |
| `list_weeks` | Optional inclusive `fromDate`/`toDate`, pagination. | Stored weeks whose date ranges overlap the filter, newest first, with IDs, date ranges, and canonical page paths. |
| `get_week` | Either `isoYear` and `isoWeek` together, or neither for the current week. | Complete existing week document plus an opaque revision token; missing plans have an explicit not-found result. |
| `search_meals` | Optional `query`, inclusive `fromDate`/`toDate`, pagination. At least a nonempty query or one date bound is required. | Matching meals with IDs, names, descriptions, types, dates, ISO weeks, and canonical week paths. |
| `get_meal` | `mealId`. | Complete saved meal including ordered recipe steps, timings, tip, nutrition, date, and canonical week path. |
| `get_shopping_lists` | Optional `date`, defaulting to today. | Resolved date/timezone, `planExists`, canonical week path, and ordered lists assigned to that date with nested categories/items. |

Use `YYYY-MM-DD` calendar dates and the existing ISO-week validation. Resolve relative language in the agent using `get_context`; the server accepts explicit dates, not natural-language date expressions. Resolve default dates once per call using the injectable server clock.

Meal search matches literal, case-insensitive substrings in meal names and descriptions, using Unicode-aware case handling. SQL wildcard characters in the query are literal text. A missing query searches all meals in the requested range. Search includes past and future plans unless constrained by dates, and orders by date descending, then stored meal position and ID. No vector database, embeddings, fuzzy matching, or synonym expansion is required.

For collection tools, use `limit` (default 20, maximum 100) and an opaque `cursor`. Return `nextCursor` explicitly, null when finished; never silently truncate. Pagination must have deterministic ordering and must not skip or repeat unchanged results. Bind cursors to the filters and reject malformed or mismatched cursors. Reject reversed date ranges and invalid dates.

## Saving plans

Expose `save_week` with an explicit ISO year/week, a complete writable week document, and `expectedRevision`. A null revision means create only if that week does not exist; a non-null revision means replace only if it matches the current saved content. Return the complete committed week and its new revision.

- The writable document covers all existing week content, including shopping lists and assignments. It excludes derived fields such as dates, totals, and `schemaVersion`. Publish its exact schema and a complete example alongside the tool documentation.
- This is full replacement within one week: omitted existing children are deleted. Tool descriptions must make that behavior explicit. To edit only a shopping list, the agent reads the week, changes that list, and submits the complete document with the revision it read. No arbitrary SQL or generic table-write tool is exposed.
- Preserve IDs for retained records and accept client-generated UUIDs for new records, including references from days to shopping lists. Reject duplicate IDs, foreign-week IDs, invalid associations, unsupported enums, non-finite or negative nutrition, and invalid calendar values. Array order defines stored sibling order; optional values are explicit nulls and empty arrays are valid.
- Validate and save the entire week in one transaction using a dedicated write connection. Retain read-only connections for page, API, and MCP read handlers. Errors roll back every change, including deletions and reordering.
- Check the revision atomically with the write. Derive revisions from all persisted week content or use equivalent tracking that also detects commits by external database writers. A counter updated only by MCP is insufficient.
- A conflict returns `revision_conflict` without changing data. The agent must fetch the current week and reconcile it; no force-overwrite parameter is provided. Retrying a stale successful request must not duplicate content or overwrite subsequent edits.
- No delete-week tool, partial-update tool family, draft/publish workflow, or UI editing is required in this iteration.

## Errors and documentation

Protocol failures use the selected MCP revision's standard error responses. Tool execution failures use an error result with a stable application code and safe message: `invalid_input`, `week_not_found`, `meal_not_found`, `revision_conflict`, or `service_unavailable`. Empty searches and shopping results are successful responses, not database failures. Never expose SQL, database paths, stack traces, or migration internals.

Treat recipe and shopping text as data, not instructions to the client. Read tools never mutate data. Document the endpoint URL, supported protocol revision, connection example, all tool input/output schemas, replacement semantics, error codes, and examples of the three user journeys. The connected agent controls its own user interaction and approval policy; Sous does not add a confirmation dialogue.

## Acceptance criteria

1. An MCP client connects, discovers all tools, and executes them through `/mcp` using the documented protocol revision. Malformed requests and unsupported protocol versions fail cleanly.
2. A fixed-clock fixture supports all three user journeys: save a new week, find a casserole from three weeks ago and retrieve its exact saved steps, and fetch only today's assigned shopping lists.
3. Context and default-date queries respect `APP_TIMEZONE` across midnight, daylight-saving transitions, and ISO year boundaries. Explicit dates remain independent of the caller's timezone.
4. Search handles multiple matches, no matches, repeated dishes across weeks, case differences, literal wildcard characters, date boundaries, and pagination without silently dropping results.
5. Shopping queries distinguish missing plans, missing day records, no assigned lists, empty lists/categories, multiple lists, and a shared list assigned to multiple days. Stored text and ordering are preserved.
6. MCP and the read API return equivalent week content. MCP recipe retrieval includes every saved meal field and its source date; updating a future copy does not modify its historical source.
7. Creating and revising a complete week preserves retained IDs, ordering, and all nested content. The next web/API read shows the committed result without restart.
8. Invalid saves leave the database unchanged. Controlled concurrent writes demonstrate atomic replacement and conflict detection, including edits through an external database connection.
9. Read tools cannot write; existing page/API mutation requests still return 405. Database failures return safe errors, and invalid origins/hosts are rejected.
10. A recorded MCP client smoke test demonstrates planning, historical recipe recall, and today's shopping query without direct database access.

## Verification

Add Rust protocol, query, and transaction tests using temporary databases and an injectable clock. Use controlled synchronization for concurrency tests. Run `cargo test` and the existing browser suite (`cargo test --test browser -- --ignored`) to verify saved content renders correctly and the read-only interface still works. Record the client/version and successful connection example before marking this feature Done.
