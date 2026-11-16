# 15. Self-contained tool schemas

## Outcome

An MCP client that cannot resolve `$ref` can still call `save_week` with a correct `document` object. Every tool's input and output schema in `tools/list` is self-contained: no `$ref` and no `$defs`. The accepted arguments and returned results do not change.

Depends on: [6. MCP meal planning and recall](006-mcp-meal-planning.md) and [10. Create weeks without a null revision](010-save-week-optional-revision.md). This feature changes only how the tool schemas are written, not what they accept.

## Background

A Claude session connected to Sous could read weeks, meals and shopping lists, but every `save_week` call failed with `invalid_input`: `invalid type: string, expected struct WeekInput`. The client sent `document` as a JSON string instead of an object.

The `save_week` input schema is valid JSON Schema 2020-12. Its `document` property is `{"$ref": "#/$defs/WeekInput", "description": ...}`, and the root of `inputSchema` has a `$defs` holding `WeekInput` and the ten types it uses ([tool list](../mcp-tools.json)). The client reported that it had no schema for `WeekInput` because the tool "doesn't include that `$defs` section". So the client either drops the root `$defs` or does not resolve `$ref`. With no type for `document`, the model encoded the week as a string.

`save_week` is the only tool whose input schema uses `$ref`; the read tools have flat inputs, which is why they work. Output schemas also use `$defs`, but reads succeed, so this client does not appear to depend on them. We do not control client schema handling, so the server should not rely on references.

The schemas come from `Tool::with_input_schema` and `Tool::with_output_schema` in rmcp 3.8. They generate draft 2020-12 schemas with schemars' default settings, which put every named type in `$defs`. schemars 1.2 has `SchemaSettings::inline_subschemas`. With it, every subschema is written in place, and `$ref` remains only for recursive types. None of the tool argument or result types is recursive.

## Scope

- Generate every tool's `inputSchema` and `outputSchema` with schemars `SchemaSettings::draft2020_12()` and `inline_subschemas = true`, in one helper in `src/mcp.rs` used by `definition`. Do not call rmcp's `with_input_schema` and `with_output_schema`.
- Keep the post-processing rmcp does today: remove the root `title` and `description`, and require root `type: "object"` for input schemas (panic at startup otherwise, as rmcp does). Keep `$schema`.
- Apply this to output schemas as well, so all seven tools follow one rule. The inlined schemas accept and reject exactly the same JSON values as the current ones; only `$ref` targets are replaced by their contents. The `nullable` helper in `src/model.rs` works unchanged, because it asks the generator for the inner schema.
- A `document` sent as a JSON string stays `invalid_input`, and nothing is written. The server does not parse a string as a fallback: the schema is the contract, and quietly accepting strings would hide client bugs.
- Tool names, titles, descriptions, annotations and the server instructions do not change.
- Regenerate the [tool list](../mcp-tools.json) (`SOUS_REGEN_TOOLS_DOC=1 cargo test --test mcp mcp_tools_doc_matches_server`).
- In the [MCP guide](../mcp.md), after the sentence that introduces the tool list, add one sentence: the schemas are self-contained (no `$ref` or `$defs`), so clients that do not resolve references still see the full `document` shape.

## Acceptance criteria

1. No tool in `tools/list` has a `$ref` or `$defs` key anywhere in its `inputSchema` or `outputSchema`.
2. The `document` property of `save_week`'s `inputSchema` is an inline object schema with `type: "object"`, `additionalProperties: false`, the required properties `week`, `days`, `shoppingLists` and `prepSessions`, and its description. The nested types are inline too: for example, a prep session's `weekday` is `["integer", "null"]` with minimum 1 and maximum 7, and is required.
3. The [example document](../examples/save-week.json) validates against `save_week`'s published `inputSchema` with `expectedRevision` omitted, `null` or a string, and fails with `expectedRevision: 7`, as in the existing test. A `document` that is a JSON string fails validation.
4. A `save_week` call whose `document` is a JSON string of a valid week returns `invalid_input` and leaves the database unchanged.
5. Every existing MCP, planning and API test passes. Tests that read `$defs` from a tool schema, such as the prep-session `weekday` schema test in `tests/mcp.rs`, read the inline schema instead and check the same properties.
6. The tool list document matches `tools/list` exactly, and the MCP guide says the schemas are self-contained.
7. Manual check, recorded before Done: in the Claude client that reported the failure, connected to a running server built from this change, save a week with `save_week`. The call succeeds with `document` sent as an object, and the week's page shows the saved plan.

## Verification

Add a test in `tests/mcp.rs` that walks every tool schema returned by `tools/list` and fails on any `$ref` or `$defs` key. Extend the `save_week` schema test to check criteria 2 and 3 against the published `inputSchema`, and add a protocol test for criterion 4. Update the `$defs`-based helpers. Run `cargo test`, then do the manual check in criterion 7.
