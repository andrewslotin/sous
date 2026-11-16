# 17. Household profile

Status: Planned; product scope agreed, detailed interface contract to be specified before implementation.

## Outcome

Sous stores household knowledge independently of weekly plans. Any connected agent can read it at the start of a fresh planning session, without relying on earlier chat history.

Depends on: [6. MCP meal planning and recall](006-mcp-meal-planning.md).

## Scope

One persistent profile for the household, stored in SQLite, with editable text sections:

- Household: people, portions and dietary restrictions.
- Food preferences: likes, dislikes and cuisines, including nuanced preferences.
- Kitchen: equipment, storage and limitations.
- Planning habits: cooking time, shopping frequency, budget, leftovers and batch preparation.
- Other context: relevant knowledge that does not fit another section.

Text preserves nuance without requiring a fixed taxonomy of preferences or equipment. Structured fields can be introduced later when a concrete feature needs them.

## Behaviour

- Extend `get_context` to return the profile alongside its existing date, timezone and week information. Agent instructions explain that it should be read before planning.
- Add an MCP tool to update individual sections. Updates must detect stale reads and prevent concurrent sessions from silently overwriting each other's changes.
- Provide a web page for users to inspect and edit the same profile that agents receive.
- Keep lasting household knowledge separate from circumstances applying only to a particular week.
- Agents should store user-stated preferences and corrections, without silently promoting inferred preferences into established facts.
- An existing agent can help bootstrap the profile by summarizing its conversation context for user review before saving it.
- The profile remains available across restarts and changes to weekly plans. Making it available does not guarantee that every client agent will fetch or follow it.

## Future extension: web feedback

Later, users of the web interface could give feedback about meals and planning outcomes. This is outside the initial feature.

Feedback should retain its relationship to the relevant meal or plan, so an agent can distinguish a particular experience from a lasting preference. A single reaction should not automatically rewrite the household profile. Feedback controls, storage and the process for incorporating repeated feedback need a separate specification.

## Acceptance criteria

1. A fresh connected agent can retrieve the saved household profile through `get_context` without access to previous conversations.
2. A section update preserves untouched sections, survives restart and does not modify weekly plans.
3. Concurrent edits produce a visible conflict rather than lost updates.
4. Users can inspect and edit the profile through the web interface, with the same conflict protection as agent edits.
5. An empty profile is a normal initial state and does not prevent meal planning.
6. The web interface follows the design system and is verified in light and dark themes.

## Details to resolve before implementation

- Exact MCP input/output schemas, revision semantics, validation limits and errors.
- Web route, navigation, empty-state wording, editor layout and save/conflict interaction.
- Browser write endpoint and request protections, since this introduces web editing.
- Migration and documentation changes, including the technical design's existing data-entry exclusion.

Implementation remains Rust with the existing frontend build pipeline; no Node dependency is needed.
