# 1. Database-backed daily plan

## Outcome

Open a known week's URL and read its meals from SQLite using the existing daily-plan design. Committed database edits appear on refresh.

Depends on: none. See the [technical design](../technical-design.md) and [visual reference](../examples/menu.html).

## Scope

- Establish the Rust/Axum/Tokio service, SQLx migrations, Serde response types, and React asset build described in the [implementation baseline](../README.md#implementation-baseline).
- Embed compiled JavaScript, styles, and reference fonts in `sous`. Serve them from the same origin, without runtime Babel or CDN dependencies.
- Create `weeks`, `days`, `meals`, and `meal_steps`, with migration bookkeeping. Include week subtitle; defer prep introduction and day reminder columns to feature 4.
- Implement the `/<year>/<week>` page and a reusable query that assembles one week's content in a single read transaction.
- Render a header, Monday–Sunday controls, meal cards, nutrition summary, descriptions, ordered instructions, time/serving badges, and tips.
- Preserve the reference spacing, typography, colors, and expandable behavior. Initially select Monday; current-day selection arrives in feature 2.
- Include only Daily Plan content at this stage; do not add nonfunctional prep or strategy tabs.

## Data and HTTP behavior

Use stable UUID text IDs, foreign keys, explicit sibling positions, and non-negative nutrition values. Days are uniquely identified within a week by weekday, rather than a position column. Missing day rows produce empty days. FOR TWO does not multiply nutrition totals.

Return 200 for an existing week, 404 for a missing or invalid week, and 503 for unavailable data. Basic error pages are sufficient here; feature 2 completes their navigation and copy. Validate ISO week existence and derive dates on the server.

Safely embed the initial JSON document into the page, escaping script-terminating content. Render database strings as text. Request connections are read-only; schema migrations use a separate writable connection. Send `Cache-Control: no-store` on dynamic pages and immutable caching on content-hashed assets.

Provide an explicitly dated development/test fixture with all four meal types, optional fields, decimal nutrition, multiple instruction steps, and an empty day. Do not seed application data automatically. The reference's Sunday–Saturday range must not be silently treated as one ISO week.

## Acceptance criteria

1. A fresh database can receive the initial migration and fixture; reapplying migrations leaves existing data intact.
2. A fixture week renders through the running binary without an external asset directory or CDN access.
3. Day navigation and meal expansion work with keyboard and pointer input; narrow mobile and desktop layouts preserve the reference design.
4. Totals sum each meal once and display at most one decimal place. Empty days omit the summary and show the specified empty message.
5. Editing a meal via another database connection appears on reload without restarting. Uncommitted changes are not displayed.
6. Invalid references, duplicate day/week identities, duplicate sibling positions, and negative nutrition values are rejected.
7. Text containing HTML and `</script>` displays safely without executing code.

## Verification

Use temporary SQLite integration tests for migrations, constraints, query ordering, nutrition, freshness, and HTTP statuses. Check the built binary's embedded assets and compare the populated daily view at mobile and desktop widths. No write API or public JSON endpoint is required yet.
