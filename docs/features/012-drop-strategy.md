# 12. Remove strategy cards

## Outcome

Sous no longer has strategy cards. The Strategy tab, the strategy tables and the `strategyCards` field are gone from the web page, the read API, MCP and the docs. Prep sessions and reminders stay as they are.

Depends on: [4. Prep, strategy, and reminders](004-weekly-content.md), [5. Read API and service verification](005-read-api.md) and [6. MCP meal planning and recall](006-mcp-meal-planning.md). This feature removes the strategy parts of feature 4 and the `strategyCards` field that features 5 and 6 expose.

## Scope

- Add migration `0004` that drops `strategy_items` and `strategy_cards`. Earlier migrations stay unchanged. Strategy cards that are already stored are deleted with the tables. All other content stays.
- Remove the Strategy tab. The plan page has two tabs: Daily Plan and Prep Sessions. Remove the "No strategy notes for this week." message and the strategy card styles.
- Remove `strategyCards` from the week document. This covers the page's embedded data, `/api/<year>/<week>` and `get_week`.
- Remove `strategyCards` from the `save_week` document. Unknown fields are already rejected, so a document that still has `strategyCards` is `invalid_input` and changes nothing.
- Set the document `schemaVersion` to `2`, because a required field is removed.
- The week revision covers only the remaining tables. Revisions read before the upgrade no longer match, so saving with one is `revision_conflict`. The client calls `get_week` again.
- Remove the strategy rows from the development fixtures.
- Routes, statuses, other page text, day selection and the rest of the document stay unchanged.

## Documentation

- Technical design: remove strategy from the feature list, the empty-state rule, the retained tabs, the retained appearance list, the week-specific text, the table list, the document shape and acceptance criterion 1. Remove "themes" from the constraints in "Data consistency". Under "Intentional changes", say that the Strategy tab of the reference page is removed. [`docs/examples/menu.html`](../examples/menu.html) stays as it is.
- OpenAPI document: remove `strategyCards`, `StrategyCard` and `StrategyItem`, update the example, and set `schemaVersion` to `2`.
- MCP guide: remove strategy cards from the save rules, the replacement list, the theme rule and the example call. Regenerate the tool listing from `tools/list` and update the tool descriptions in the server.
- Save example: remove `strategyCards`.
- README: remove strategy cards from the overview.
- Features 1–11 stay as history. This spec records the removal.

## Acceptance criteria

1. A database that has strategy cards upgrades without errors. Afterwards `strategy_cards` and `strategy_items` do not exist, and every week's days, meals, shopping lists, prep sessions and reminders are unchanged.
2. The plan page shows Daily Plan and Prep Sessions tabs only. No strategy text, message or style remains in the app.
3. The embedded page data, `/api/<year>/<week>` and `get_week` have no `strategyCards` and have `schemaVersion: 2`. The embedded document still equals the API response.
4. `save_week` with a document that has `strategyCards` returns `invalid_input` and changes nothing. The same document without it saves.
5. A revision from `get_week` after the upgrade replaces the week. A revision computed before the upgrade is `revision_conflict`.
6. The OpenAPI document, the MCP tool listing and the save example match the code, and no doc outside `docs/features/` describes strategy cards as current behaviour.

## Verification

Add a migration test that applies migrations `0001`–`0003`, stores a week with strategy cards and other content, runs `0004`, and compares the remaining week document with the one before, apart from `strategyCards` and `schemaVersion`. Update the HTTP, MCP and save tests: remove the strategy cases and add the `invalid_input` case for `strategyCards`. The existing OpenAPI, tool listing and example checks cover the docs. Update the browser tests (`cargo test --test browser -- --ignored`) to expect two tabs, and capture the plan page at 375 px and 1280 px.
