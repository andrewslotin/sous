# Sous feature tracker

The [technical design](technical-design.md) defines the service contract. The [example page](examples/menu.html) defines the visual reference.

Implement features in the order below. Each iteration includes its own verification and leaves a usable service. Status values: Planned, In progress, Done. Mark a feature Done when its acceptance criteria pass.

| # | Feature | Status | Depends on |
| --- | --- | --- | --- |
| 1 | [Database-backed daily plan](features/001-database-backed-daily-plan.md) | Done | — |
| 2 | [Calendar navigation and archive](features/002-calendar-navigation.md) | Done | 1 |
| 3 | [Shopping lists](features/003-shopping-lists.md) | Done | 2 |
| 4 | [Prep, strategy, and reminders](features/004-weekly-content.md) | Planned | 3 |
| 5 | [Read API and service verification](features/005-read-api.md) | Planned | 4 |

## Implementation baseline

- Rust binary named `sous`.
- Axum and Tokio for HTTP handling; SQLx for SQLite queries and versioned migrations; Serde for serialization.
- Runtime-checked SQL queries, verified against temporary SQLite databases in integration tests. Compilation does not require a database or checked-in SQLx query metadata.
- Existing React presentation, compiled at build time and embedded with its assets in the binary.
- Separate HTTP handlers, application queries, and persistence code without a generic repository framework.
- Introduce migrations and fixture-based verification in feature 1; extend them with each feature.

These specs cover implementation of the service only. Data-entry tools, deployment procedures, and MCP are outside scope.
