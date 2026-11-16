---
name: implement-feature
description: Implement one Sous feature spec (docs/features/NNN-*.md) end to end. Sonnet subagents write the code, Claude verifies it, and Codex reviews it over agterm until both agree it is complete. Use when asked to implement a feature from docs/features/.
argument-hint: <path to docs/features/NNN-*.md>
---

# Implement a feature with Sonnet workers and a Codex reviewer

You orchestrate the work and verify it. Sonnet subagents write the code. Codex runs in another agterm pane and reviews it.
Keep going through the whole loop without stopping between steps. Stop only for a decision that belongs to the user.

## 1. Load context

Read these before planning anything:
- the feature spec passed as the argument. Its acceptance criteria are binding.
- `docs/README.md` (feature tracker) and `docs/technical-design.md`.
- `tasks/lessons.md`. Apply every lesson in it.
- the NEXT feature spec, but only to find where this feature's scope ends. Do not build anything it owns.
  Example: feature 1 returned a plain 404 for `/2026/04` because feature 2 owns canonical redirects.
- the code that exists now (`src/`, `migrations/`, `tests/`, `web/`, `build.rs`).

Settled project decisions. Do not ask about them again:
- **Pure Rust build.** There is no Node, npm, bun or deno, and none may be installed. `build.rs` compiles `web/app.jsx`
  with oxc using the classic runtime. React 18.3.1 UMD and the fonts are vendored in `web/` and embedded in the binary.
- **Running code.** Agents and you may run `cargo build`, `cargo test`, `cargo clippy` and `cargo fmt`. Never start the
  `sous` server binary; ask the user to do visual checks. An in-process server inside a test is fine.
- **Browser tests.** They live in `tests/browser.rs` and use the `headless_chrome` crate with Google Chrome. They are
  `#[ignore]`d and run with `cargo test --test browser -- --ignored`.

## 2. Write the brief

Write a brief to the scratchpad (`brief-NNN.md`), never into the repo. Subagents start with no context, so it must
stand on its own:
- **Docs to read:** the spec and the technical design. For any change to `web/`, also `docs/design-system.md`, the
  visual reference. Its tokens, type, shapes and component rules bind the worker. If the feature needs something
  the design system lacks, the spec must say how to extend it, and one owner updates `docs/design-system.md`.
- **Ground rules:** cargo commands allowed; never run the server; never commit; never edit `docs/`, except the docs the
  spec says to update, each assigned to one owner (the tracker stays yours). Use runtime
  `sqlx::query`/`query_as` and no `query!` macros. Do not build later features' scope. Write files with the
  Write/Edit tools, not shell heredocs, which a Bash hook may reject.
- **Ownership:** which agent owns which files. Agents that run in parallel must never share a file, and that includes
  sections of `Cargo.toml`: one agent owns `[dependencies]`, another owns `[build-dependencies]`.
- **The contract between agents:** exact Rust signatures and the JSON document shape, extended from the existing
  `src/model.rs`. Pin any fixture rows (ids, positions, text) that more than one agent's tests rely on.
- **Tasks:** a numbered list per agent. Quote spec phrases verbatim and take the narrowest reading of an ambiguous
  one. Feature 2's "available-week links" grew into a full week list that the user then had removed.
- **Required tests:** map every acceptance criterion to at least one test. Use temporary SQLite databases for
  integration tests and `tests/browser.rs` for anything the user sees: rendering, keyboard/pointer, layout widths,
  text safety. When the feature has visual criteria, also have a browser test write full-page screenshots at 375
  and 1280 px, in light and in dark, to `target/visual/`. Look at them in step 4 and compare them with
  `docs/design-system.md`. Brief workers to write files only with Write/Edit,
  never `cat`/heredoc appends. For migration-preservation tests, keep earlier features' fixture files frozen and put new content in
  a separate fixture, so the upgrade test loads real earlier-iteration data into the old schema.

When the frontend's browser tests need the new backend behaviour, run the backend agent first and the frontend agent
after it, with no stubs. Otherwise, if the agents will run in parallel, first land any stubs the contract needs yourself, so the crate compiles on its
own. Before you start, tell the user in one line that this stub step is the skill's own and that the workers write
all logic and tests. Keep stubs to signatures and `todo!()`. Run `cargo check` and launch the workers as soon as it
passes.

## 3. Run Sonnet workers

Launch them with `Agent`, `subagent_type: general-purpose`, `model: sonnet`, in the background. Use one agent per
independent area, for example backend and frontend. Each prompt should:
- point the agent at the brief and name its role;
- forbid progress text: "Keep calling tools without sending text; your only text message is the final report";
- require `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` to pass before it reports;
- ask for a report covering the files it changed, decisions beyond the brief, exact test counts and open issues.

**Expect agents to stop early** without a report, often after 6–15 tool calls. This is normal. Do not restart them.
For each stop:
1. Check what exists with `git status --short`, `wc -l` on the owned files and `ls tests`.
2. Resume the same agent with `SendMessage`. Name exactly what is done and what is left, and repeat "do not end
   your turn until done; your only text message is the report".

Repeat until the agent sends its hand-back report. Treat each report as a claim to verify, not as the result.

## 4. Verify the work yourself

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo test --test browser -- --ignored
```

Read the key new code: the migration, the handlers, the queries and the changed JSX/CSS. Compare it with the spec
and the reference, and catch scope creep or gaps before asking Codex to review.

Sweep the docs, not just the files the spec names. Grep the README and `docs/` (design, OpenAPI,
MCP guide) for every route, payload and test name the feature changed. Fix stale current-contract references.

## 5. Ask Codex to review

Find Codex's pane. It is usually the split pane of this session (`splitForeground` contains `codex`). If the user
names a pane UUID, address it with `--pane-id <uuid>` (`--pane` takes only left/right/scratch), and give Codex your
own pane UUID for its reply (`agtermctl tree --json | grep -o '"paneID":"[^"]*"'`). Check it is idle before typing.
If it shows an approval prompt, do not type (keystrokes would answer it): ask the user to resolve it first.

```bash
agtermctl tree --json --socket "$AGTERM_SOCKET"     # find the session or pane running codex
agtermctl session text --target "$AGTERM_SESSION_ID" --pane right --lines 20 --socket "$AGTERM_SOCKET"
```

Write the request as ONE line, because a newline submits early. Include:
- what changed and where, and the test results;
- scope decisions Codex should not flag, such as work that belongs to a later feature;
- doc coverage: which docs changed and what the step 4 sweep checked;
- what to review against: the spec path and `docs/technical-design.md`;
- "Do not edit source files";
- **how to reply**: over agterm into your pane, never in a file. Codex should run
  `agtermctl session type "<findings or APPROVED>" --target <your $AGTERM_SESSION_ID> --pane left`, then send Enter
  as a separate `agtermctl session type` call, keeping the reply on one line.
  Write your actual session id into the message, not the variable.

Write the request to `codex-review-N.txt` in the scratchpad with the Write tool, and type it from that file. A
heredoc counts as inline. Never inline it: a Bash
hook blocks any command whose text contains version-control words. Do all of the following **in the same turn**.
Type the text, re-read the pane until it shows `[Pasted Content …]`, send Enter as a separate call, then confirm:

```bash
agtermctl session type "$(cat <scratchpad>/codex-review-N.txt)" --target "$AGTERM_SESSION_ID" --pane right --socket "$AGTERM_SOCKET"
agtermctl session text --target "$AGTERM_SESSION_ID" --pane right --lines 5 --socket "$AGTERM_SOCKET"   # until the paste shows
agtermctl session type $'\r' --target "$AGTERM_SESSION_ID" --pane right --socket "$AGTERM_SOCKET"
agtermctl session text --target "$AGTERM_SESSION_ID" --pane right --lines 20 --socket "$AGTERM_SOCKET"
```

The request went through when the input box is empty and Codex shows `Working`. Enter gets lost in three cases: a
`\r` inside the same `type` call as the text, an Enter sent before the paste registers, and a turn that ends before
Enter is sent. `Working` can take a few seconds to show: read the pane once more before concluding Enter was lost.
Then end your turn: Codex's verdict arrives as the next message in this conversation.

## 6. Iterate until both agree

Treat each finding as a claim to check. Look at the evidence: screenshots, file and line references, reproduction
steps. If you disagree, reply to Codex over agterm with your reasoning, using the same send-and-Enter procedure.
For each accepted finding:
1. Give the fixes to a Sonnet agent with a precise brief. Require a regression test, and require proof that the
   test **fails without the fix**: revert the fix temporarily, run the test, then restore it. A docs-only wording
   fix needs no agent or regression test: make it yourself, sweep the file for the same wording, and rerun the
   doc's test (e.g. `openapi_matches_api`).
2. Run the step 4 checks again yourself.
3. Ask Codex to re-review. Summarise each fix and its evidence, and include the reply-via-agterm instructions again.

The loop ends when Codex replies APPROVED and you have mapped every acceptance criterion to passing evidence.
That evidence is a test, or a check Codex did in a browser.

## 7. Finish

- Set the feature's status to `Done` in the `docs/README.md` tracker. If the spec requires a manual check (e.g. the
  user's visual check) before Done, set `In progress` and mark Done only after the user reports the result.
- Add every correction the user made this session to `tasks/lessons.md`, as a rule that prevents a repeat.
- Report to the user:
  - what was built;
  - the review rounds and their findings;
  - the exact test results;
  - what still needs the user, such as a visual check with `make run`, since you never run the server. Suggest
    `make reset` only when a migration or fixture changed.
- **Commit only when the user asks.** Create a branch first, unless the user names one (e.g. "commit on main"). Use a single-line message with no Claude attribution and
  no Co-Authored-By. Fast-forward `main` or open a PR only when asked. Stage the session's files by name, never
  `git add -A`: the user may have uncommitted edits of their own, which stay out unless they say otherwise.
