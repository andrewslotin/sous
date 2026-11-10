# 10. Create weeks without a null revision

## Outcome

An MCP client can create a new week with `save_week` even if it cannot send a JSON `null`. Leaving `expectedRevision` out means "create". `null` keeps working, and a revision string still replaces the week only when it matches.

Depends on: [6. MCP meal planning and recall](006-mcp-meal-planning.md). This feature changes the `expectedRevision` rule in that spec's "Saving plans" section.

## Background

Some MCP clients cannot send JSON `null` for a top-level argument whose schema type is `["string", "null"]`. The Claude Code client is one of them. These clients send the text `"null"` instead. The server reads that as a revision for a week that does not exist and returns `revision_conflict`. Such a client cannot create a week at all. The error message then says the week "changed since it was read, or already exists". Neither is true here, so the failure is hard to diagnose.

## Scope

- `expectedRevision` is no longer in the required list of the `save_week` input schema. Its type stays `["string", "null"]`.
- An omitted `expectedRevision` behaves exactly like `null`. It creates the week if no plan is saved for that ISO week. If a plan is already saved, it returns `revision_conflict` and changes nothing.
- A revision string replaces the week only if it matches the stored week's current revision. Otherwise the call returns `revision_conflict`. This is unchanged.
- The text `"null"` gets no special handling: it is an ordinary revision string.
- `revision_conflict` messages say which case occurred. The code stays `revision_conflict`, and the messages never expose internals.

| Situation | Message |
| --- | --- |
| Create requested (omitted or `null`) but the week is already saved | `A plan is already saved for this week. Fetch it with get_week and resubmit with its revision.` |
| Revision string sent, no plan saved for the week | `No plan is saved for this week. Omit expectedRevision to create it.` |
| Revision string does not match the stored week | `The week changed since it was read. Fetch it with get_week and resubmit.` |

- Every other `save_week` rule is unchanged: full replacement, one transaction, required document fields with explicit `null` for optional values, ID rules and validation codes. The relaxation applies only to the top-level `expectedRevision` argument, not to fields inside `document`.
- Update the `save_week` tool description and the server `instructions` so both say: omit `expectedRevision` to create a week, pass the revision from `get_week` to replace one. Leave the other tools unchanged.
- Update the [MCP guide](../mcp.md) (the tools table, the `expectedRevision` rules and the "Plan a new week" example, which should omit the argument), the [tool list](../mcp-tools.json), the technical design if it states the rule, and the [verification record](../verification.md).

## Acceptance criteria

1. `save_week` without `expectedRevision` creates a week that does not exist. The result and the next `get_week` and web/API reads show the committed document.
2. `save_week` without `expectedRevision` for a saved week returns `revision_conflict` with the "already saved" message and leaves the database unchanged.
3. `save_week` with `expectedRevision: null` behaves as it did before this feature, for both a new week and a saved week.
4. `save_week` with a revision string for a week with no plan, including the string `"null"`, returns `revision_conflict` with the "no plan is saved" message and creates nothing.
5. A matching revision string still replaces the week, and a stale one returns `revision_conflict` with the "changed since it was read" message. Existing concurrency and external-writer conflict tests still pass.
6. The `tools/list` schema for `save_week` does not list `expectedRevision` as required, and the tool list document still matches `tools/list` exactly.
7. The tool description, server instructions and MCP guide describe omitting `expectedRevision` to create a week. No current-contract text says `expectedRevision` is required.
8. The recorded MCP client smoke test creates a week without `expectedRevision`.

## Verification

Extend the MCP protocol tests with temporary databases to cover each acceptance criterion, including the `"null"` string case. Keep the existing null-revision tests. Run `cargo test`. Manual check, to be recorded before Done: in a Claude Code session connected to a running server, create a week that does not exist yet with `save_week` and no `expectedRevision`, then confirm the week appears on its page.
