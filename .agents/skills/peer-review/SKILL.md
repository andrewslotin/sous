---
name: peer-review
description: Review a Sous feature implemented by Claude in another agterm pane. Verify the implementation against its spec, inspect and run necessary tests, send findings over agterm, and re-review fixes until no blocking issues remain. Use for the reviewer side of the project's implement-feature workflow.
---

# Peer review

Act as the independent reviewer; Claude owns implementation and fixes. This is the counterpart to `.claude/skills/implement-feature/SKILL.md`. Review the shared working tree without editing source files, migrations, tests, or feature status. Do not commit or launch additional agents unless the user requests it. Temporary review artifacts belong outside the repository.

## Establish the handoff

Read the applicable repository instructions, `tasks/lessons.md`, the requested feature spec, `docs/README.md`, and relevant sections of `docs/technical-design.md`. For presentation changes, also inspect `docs/examples/menu.html`. Use acceptance criteria to define the review scope. If the scope boundary is ambiguous, explain why you need to read the adjacent feature spec, then read only what resolves the ambiguity; do not require future-feature behavior now.

Use the available agterm skill for terminal addressing and communication. Discover the live session and panes with `agtermctl tree --json`; identify Claude and Codex from their foreground processes and repository paths. Do not assume Claude is always left, Codex is always right, or that `active` is your session. Prefer discovered stable pane IDs. When `AGTERM_SOCKET` is set, pass `--socket "$AGTERM_SOCKET"` on every call. If session environment variables are missing, use the discovered explicit IDs.

If Claude is still implementing, prepare by reading the spec and existing tests; wait for its ready-for-review handoff before treating unfinished code as final. Do not submit text into a pane containing an unsent user prompt. Ask only if the intended peer cannot be identified safely.

The handoff should identify the spec, changed files or working tree, claimed verification, intentional scope decisions, and reply destination. Infer available details from the repository and live panes rather than repeatedly asking. No fixed session UUID or absolute repository path belongs in this skill.

## Review independently

Inspect `git status`, the diff, and relevant untracked implementation files. The implementation may be entirely uncommitted; a clean tracked diff alone is not evidence there is nothing to review. Avoid racing ongoing edits or formatting the shared tree.

Trace each acceptance criterion to implementation and meaningful evidence. In particular:

- Check persistence constraints, ordering, transaction boundaries, committed-write visibility, read-only request connections, status codes, and cache headers when affected.
- Check actual rendered behavior for totals, decimal formatting, empty states, keyboard/pointer controls, escaping, and narrow/desktop layout when affected. JSON assertions do not prove the frontend calculates or displays the right result; checking that JavaScript contains strings does not prove it runs.
- Inspect whether tests would catch the relevant failure. For fixes, look for a regression test that fails without the fix. Do not revert Claude's shared files yourself to prove this; inspect its evidence or use an isolated copy if needed.
- Distinguish concrete correctness or acceptance gaps from optional style preferences. A missing test is actionable when it leaves required behavior unverified, not merely because every line lacks a test.

Respect the project baseline: the frontend builds through Rust/oxc with vendored assets. Do not introduce Node or another JavaScript toolchain. Do not launch the standalone `sous` server; in-process servers in tests are allowed. If a visual check cannot be performed through the existing browser harness, report the gap and request the user's visual check rather than silently starting the server.

Run the checks relevant to the change, normally:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test browser -- --ignored
```

Use `fmt --check`, not a formatting command that changes files. The browser suite is ignored by default: a green `cargo test` does not mean browser tests ran. Inspect the browser harness and its requirements before invoking it. Use temporary databases and isolated browser profiles. Follow environment approval requirements for browser launches or other restricted operations; a blocked check is not a passing check.

Record exact commands, results, and any material verification gaps. Do not broaden or repeat successful checks without a changed implementation, failure, or unresolved concern.

## Deliver findings through agterm

For each actionable finding, give severity (P0–P3), file:line, trigger, observed impact, the violated requirement, and a concrete fix/test expectation. Rank findings by severity. Avoid speculative issues and demands to implement later features. Say `APPROVED - no blocking issues` only when the current scope is supported by sufficient evidence; distinguish optional suggestions from blockers.

Send a concise ONE-LINE verdict to the verified Claude pane. Invoking this workflow authorizes review messages to that implementation peer, not unrelated recipients. Use safe shell quoting or an argument-safe tool call for message text, especially text containing backticks, dollar signs, or quotes.

The transport sequence is essential:

1. Read the target pane and confirm it is the intended agent with an empty input box ready for the handoff.
2. Type the single-line verdict, without a newline.
3. Send Enter in a SEPARATE `session type` call in the SAME assistant turn.
4. Read the pane again to verify submission. Do not assume an `ok` response means the TUI submitted the message. If delivery is uncertain, inspect before retrying to avoid duplicate messages.

Example commands, with variables set to the discovered destination and safely prepared message:

```bash
agtermctl session type "$REVIEW_MESSAGE" --target "$PEER_SESSION" --pane-id "$PEER_PANE"
agtermctl session type $'\r' --target "$PEER_SESSION" --pane-id "$PEER_PANE"
agtermctl session text --target "$PEER_SESSION" --pane-id "$PEER_PANE" --lines 20
```

Add the socket option when set. Execute the text and Enter as separate tool calls, completing both before ending the turn. Do not put literal multiline findings into terminal input. For lengthy evidence, keep the verdict concise and link a temporary artifact; the notification still goes through agterm. Use a review file as the primary handoff only if explicitly requested, and honor any requested completion marker.

## Re-review and finish

After sending findings, yield for Claude's fix handoff instead of repeatedly polling files or interrupting active implementation. On return, inspect the fixes, added tests, and any new changes. Re-run affected checks; verify that prior blockers are resolved. Evaluate disagreements against the spec and evidence, and withdraw a finding if it was wrong.

Continue the review/fix loop until there are no blocking issues. If a required decision or unavailable verification prevents approval, clearly state what remains and who can resolve it; do not keep cycling on the same blocker. Approval is a review verdict, not permission to commit, merge, deploy, or change the feature tracker.

Send the final verdict over agterm with the same separate-Enter procedure. Then give the user a short self-contained result: approved or changes requested, key findings, checks run, and unresolved limitations. Keep source ownership with Claude throughout.
