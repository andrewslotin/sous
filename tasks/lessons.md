# Lessons

- Messaging Codex (or any TUI agent) via `agtermctl session type`: send the text, then send Enter as a
  SEPARATE call. A `\r` typed in the same burst is swallowed as part of the paste. Afterwards read the
  pane with `session text` and confirm the input box is empty before assuming it was submitted.
- Getting results back from Codex: ask it to reply via agterm (`agtermctl session type --target <my session> --pane left`,
  text first, Enter as a separate call), NOT by writing a file that I poll.
- Send the text and the separate Enter in the SAME turn (two Bash calls back to back, or `type text; type $'\r'` with
  the verify step between). Never end a turn with only the text typed: a hook can stop continuation between calls.
