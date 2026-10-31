# Lessons

- Messaging any TUI agent with `agtermctl session type`: type the text, then send Enter as a SEPARATE call in the same
  turn, and read the pane back to confirm it was submitted. To get answers back, ask the agent to reply over agterm,
  not by writing a file that I poll. The Codex-specific steps are in the implement-feature skill, step 5.
- When a tool call fails, or a hook stops or blocks one, tell the user at once in one line: what fired and what I'll
  do next. Never end a turn silently. Keep exploratory command chains short (at most three calls before a status
  line), and post a one-line status after loading context (before writing a brief) and before a multi-call handoff
  such as typing a review request.
- Change files with Edit/Write, never shell heredocs or inline scripts. Run a multi-line script from a scratchpad
  file: multi-line inline Bash (`python3 -c`, heredocs) can trip the commit-message hook.
- Before reading something outside the task (e.g. the next feature's spec), say why in one line.
- Name artifacts precisely: "agent brief (scratchpad)", not "spec".
- Check the toolchain against the baseline before planning. Raise a missing tool once, with a recommendation.
- Verify facts before putting them in an agent brief; agents copy mistakes. Check worker-written docs (OpenAPI
  ranges, formats, descriptions) against the code too, before review.
- Axum extractors reject bad input before the handler runs. Where the spec defines the error for a bad path or
  query, take the extractor as `Result<_, Rejection>` and test malformed input such as `%FF`.
- When the user remarks on behaviour, first quote the spec line it implements and ask whether to keep it. Don't
  propose redesigns or launch changes from an observation. The spec is the default answer.
- When the user asks for a change, make that change and nothing more. Don't propose alternatives ("nearest weeks").
  If the instruction can be read two ways, or conflicts with the spec, ask before dispatching anything. Update the
  spec when the user says so.
- Keep a shared control in the same place on every page kind (e.g. the "All weeks" link in the header row), so
  pages stay consistent with the plan page.
- When the spec is silent on how an empty child collection displays (e.g. a prep session with no tasks), ask the
  user before building it. Don't settle it myself and list it as a scope decision.
