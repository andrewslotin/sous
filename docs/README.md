# Sous feature tracker

The [technical design](technical-design.md) defines the service contract. The [example page](examples/menu.html) defines the visual reference. The [OpenAPI document](openapi.json) describes the read API, and the [verification record](verification.md) maps acceptance criteria to tests and manual checks.

Implement features in the order below. Each iteration includes its own verification and leaves a usable service. Status values: Planned, In progress, Done. Mark a feature Done when its acceptance criteria pass.

| # | Feature | Status | Depends on |
| --- | --- | --- | --- |
| 1 | [Database-backed daily plan](features/001-database-backed-daily-plan.md) | Done | — |
| 2 | [Calendar navigation and archive](features/002-calendar-navigation.md) | Done | 1 |
| 3 | [Shopping lists](features/003-shopping-lists.md) | Done | 2 |
| 4 | [Prep, strategy, and reminders](features/004-weekly-content.md) | Done | 3 |
| 5 | [Read API and service verification](features/005-read-api.md) | Done | 4 |
| 6 | [MCP meal planning and recall](features/006-mcp-meal-planning.md) | Done | 5 |
| 7 | [Year archive pages](features/007-year-archive.md) | Done | 2 |
| 8 | [API routes matching page routes](features/008-api-routes.md) | Done | 7 |
| 9 | [Docker image and CI](features/009-docker-image.md) | In progress | — |
