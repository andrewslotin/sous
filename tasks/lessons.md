# Lessons

- Messaging Codex (or any TUI agent) via `agtermctl session type`: send the text, then send Enter as a
  SEPARATE call. A `\r` typed in the same burst is swallowed as part of the paste. Afterwards read the
  pane with `session text` and confirm the input box is empty before assuming it was submitted.
- Getting results back from Codex: ask it to reply via agterm (`agtermctl session type --target <my session> --pane left`,
  text first, Enter as a separate call), NOT by writing a file that I poll.
- Send the text and the separate Enter in the SAME turn (two Bash calls back to back, or `type text; type $'\r'` with
  the verify step between). Never end a turn with only the text typed: a hook can stop continuation between calls.
- When a tool call fails, say so and continue. Don't end the turn silently. Keep exploratory command chains short.
- Before reading something outside the task (e.g. the next feature's spec), say why in one line.
- Name artifacts precisely: "agent brief (scratchpad)", not "spec".
- Check the toolchain against the baseline before planning. Raise a missing tool once, with a recommendation.
- Verify facts before putting them in an agent brief; agents copy mistakes.
- implement-feature: keep orchestrator stubs to signatures + `todo!()` and say up front, in one line, that the stub
  step is the skill's and Sonnet workers write all logic and tests. Write the brief and launch workers right after
  `cargo check` passes; don't let stub work drift into implementation.
- If a hook stops continuation or blocks a call, tell the user at once in one line (what fired, what I'll do next)
  instead of ending the turn silently. The Bash hook here blocks commands whose text mentions version-control words:
  type agterm messages from a scratchpad file with `agtermctl session type "$(cat file)"`, and edit docs with Edit.
- Codex's TUI registers long pastes late: after typing, re-read the pane until it shows `[Pasted Content …]`, then
  send Enter, then confirm `Working`. An Enter sent too early is lost.
- When the user remarks on behaviour, first quote the spec line it implements and ask whether to keep it. Don't
  propose redesigns or launch changes from an observation. The spec is the default answer.
- When the user asks for a change, make that change and nothing more. Don't propose alternatives ("nearest weeks").
  Ask only when the instruction conflicts with the spec, then update the spec when the user says so.
- Keep a shared control in the same place on every page kind (e.g. the "All weeks" link in the header row), so
  pages stay consistent with the plan page.
