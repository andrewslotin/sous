# Sous

- Specs and the feature tracker are in `docs/`. Implement features with the `implement-feature` skill.
- Frontend is pure Rust: `build.rs` compiles `web/app.jsx` with oxc. There is no Node, and none should be added.
- The UI follows `docs/design-system.md`. Read it before you change anything in `web/`.
  - Use only its tokens (`var(--…)`), never raw colours. No emoji in the interface's own text. Keep each primary with its shape: a blue circle for Sous, a red square for shopping, a yellow triangle for prep.
  - Check every visual change in light and in dark.
  - If a change needs something the design system lacks, update `docs/design-system.md` in the same change.
- Tests: `cargo test`; browser tests: `cargo test --test browser -- --ignored`.
- Read `tasks/lessons.md` at session start.
