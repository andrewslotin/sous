# Sous feature tracker

The [technical design](technical-design.md) defines the service contract. The [design system](design-system.md) defines the visual reference; the [example page](examples/menu.html) is kept as history. The [OpenAPI document](openapi.json) describes the JSON API, including the household profile endpoint.

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
| 9 | [Docker image and CI](features/009-docker-image.md) | Done | — |
| 10 | [Create weeks without a null revision](features/010-save-week-optional-revision.md) | Done | 6 |
| 11 | [Calendar sidebar](features/011-calendar-sidebar.md) | Done | 7 |
| 12 | [Remove strategy cards](features/012-drop-strategy.md) | Done | 6 |
| 13 | [Day view](features/013-day-view.md) | Done | 11, 12 |
| 14 | [Redesign](features/014-redesign.md) | Done | 13 |
| 15 | [Self-contained tool schemas](features/015-inline-tool-schemas.md) | In progress | 6, 10 |
| 16 | [Today link](features/016-today-link.md) | Done | 14 |
| 17 | [Household profile](features/017-household-profile.md) | Done | 6 |
| 18 | [Read-first household page](features/018-household-read-first.md) | Done | 17 |
| 19 | [Back link on the household page](features/019-household-back-link.md) | Done | 18 |
| 20 | [Shopping checkboxes](features/020-shopping-checkboxes.md) | Done | 14 |
