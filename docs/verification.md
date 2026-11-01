# Sous: verification record

Maps every acceptance criterion in the [technical design](technical-design.md#verification-and-acceptance-criteria) and the [read API spec](features/005-read-api.md#acceptance-criteria) to automated tests or recorded manual checks.

- Backend tests: `tests/backend.rs` and unit tests in `src/`, run with `cargo test`.
- Browser tests: `tests/browser.rs`, run with `cargo test --test browser -- --ignored` (headless Google Chrome).
- Screenshots: `visual_capture_at_375px` and `visual_capture_at_1280px` write `target/visual/<width>-<name>.png`.
- API contract: [`openapi.json`](openapi.json), checked against live output by `openapi_matches_api`.

## Technical design

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | Populated fixture renders all tabs, cards, text, badges, instructions and strategy themes with the reference styling, on desktop and mobile, including expanded states | Browser: `monday_selected_with_exact_fixture_totals`, `tabs_switch_and_keep_selected_day`, `prep_sessions_render_and_toggle`, `strategy_cards_use_theme_colors_in_order`, `shopping_content_matches_fixture_and_reloads_changes`, `summary_layout_at_375px`/`1280px`, `shopping_layout_at_375px`/`1280px`, `visual_capture_at_375px`/`1280px`. Manual: visual check below. |
| 2 | `/` resolves the current ISO week in the configured timezone; `/2026/44` shows October 26–November 1, 2026 | Backend: `root_selects_today_in_service_timezone`, `week_page_payload_and_dates`. Browser: `plan_header_range_and_all_weeks_link_by_keyboard`, `current_day_comes_from_server_clock_not_browser_timezone`. |
| 3 | ISO year boundaries, valid/invalid week 53, timezone midnight boundaries, canonical redirects, old/future week selection | Backend: `iso_week_53_and_year_boundary`, `root_selects_today_in_service_timezone`, `zero_padded_weeks_redirect_permanently`, `api_zero_padded_redirects`, `explicit_week_selects_today_only_in_current_week`, `invalid_paths_are_404_invalid_and_never_500`. Browser: `year_boundary_week_selects_saturday`, `past_week_selects_monday_and_keeps_url`, `invalid_week_page_and_zero_padded_redirect`. |
| 4 | Missing current plans, missing explicit weeks, empty archives, missing days and unavailable databases produce their messages and statuses | Backend: `root_missing_current_week_is_200_with_summary`, `explicit_missing_week_is_404_without_weeks`, `year_status_empty_database_and_current_year`, `http_503_when_database_unavailable`, `api_error_contract`. Browser: `missing_week_pages_link_to_archive_only`, `empty_database_pages`, `day_switching_empty_states_and_restore`, `empty_prep_and_strategy_show_messages`, `database_failure_shows_unavailable_not_missing`. |
| 5 | A meal edited through a separate connection is visible on refresh without restart; an uncommitted transaction is never partially rendered | Backend: `freshness_commit_visible_uncommitted_not`, `api_freshness_after_commit`, `api_snapshot_is_atomic_under_concurrent_commit`, `weekly_content_freshness`, `shopping_content_order_and_freshness`. Browser: `shopping_content_matches_fixture_and_reloads_changes`. |
| 6 | Shopping lists render categorized bullets with no checkboxes or progress; collection ordering stays stable after edits | Browser: `shopping_items_are_plain_bullets`. Backend: `shopping_day_order_follows_link_position`, `shopping_content_order_and_freshness`, `weekly_content_order_and_reorder`, `query_shape_and_ordering`. |
| 7 | Nutrition totals sum stored values once per meal regardless of FOR TWO; deterministic decimal formatting | Backend: `nutrition_round_trips_as_decimals`. Browser: `monday_selected_with_exact_fixture_totals`, `sums_before_rounding_and_trims_trailing_zero`. |
| 8 | Constraints reject invalid references, duplicate sibling positions and cross-week shopping associations | Backend: `constraints_reject_bad_data`, `shopping_constraints_reject_bad_data`, `weekly_constraints_reject_bad_data`. |
| 9 | User text is escaped in React rendering and in initial JSON embedding | Backend: `script_safety`, `weekly_content_script_safety`; unit: `week_page_escapes_title_and_uses_only_local_assets`. Browser: `script_terminating_names_render_as_text`, `shopping_text_renders_literally`. |
| 10 | Pages render without third-party CDN access | Backend: `page_references_only_same_origin_assets`. Browser: `no_third_party_requests_on_any_page`. |

## Feature 5: read API

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | HTML bootstrap data and API JSON describe the same fixture week | Backend: `api_matches_page_bootstrap`, `openapi_matches_api`. |
| 2 | Error cases have the specified status, content type and JSON code | Backend: `api_error_contract`, `openapi_matches_api`. |
| 3 | A concurrent transaction cannot produce a response mixing old and new records (controlled synchronization) | Backend: `api_snapshot_is_atomic_under_concurrent_commit`. It fails when the post-pause queries run outside the read transaction. |
| 4 | The next request after commit observes updated data | Backend: `api_freshness_after_commit`, `freshness_commit_visible_uncommitted_not`. |
| 5 | Mutation methods return 405; database errors do not leak | Backend: `non_get_methods_are_405`, `api_mutations_405_and_no_leaks`. |
| 6 | The binary serves its complete interface with no third-party requests; missing assets return 404, not a plan page | Browser: `no_third_party_requests_on_any_page`. Backend: `missing_assets_are_plain_404`; unit: `unknown_asset_is_404`. |
| 7 | Mobile/desktop visual verification of all tabs, expanded cards, empty states and archived/current week selection | Browser: `visual_capture_at_375px`/`1280px` (asserts selections and empty states, writes screenshots). Manual: visual check below. |
| 8 | Every technical-design criterion is covered | This document. |

## Feature 6: MCP meal planning

Protocol tests are in `tests/mcp.rs`; data-layer tests are in `tests/planning.rs`. Client setup and tool semantics
are in the [MCP guide](mcp.md), and `mcp_tools_doc_matches_server` keeps the [tool list](mcp-tools.json) in step with the server.

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | A client discovers and runs every tool; malformed requests and unsupported protocol versions fail cleanly | MCP: `stateless_flow_lists_seven_tools_and_calls_them`, `legacy_initialize_flow_works`, `server_info_names_sous_and_warns_that_text_is_data`, `malformed_and_unsupported_requests_fail_cleanly`, `argument_errors_are_tool_errors`, `mcp_tools_doc_matches_server`. |
| 2 | The three user journeys work on a fixed-clock fixture | MCP: `journey_plan_recall_and_shop`, `save_week_example_round_trips`. |
| 3 | Defaults follow `APP_TIMEZONE` across midnight, DST and ISO year boundaries; explicit dates do not | MCP: `defaults_follow_the_service_timezone`, `explicit_dates_ignore_the_clock_timezone`. |
| 4 | Search: multiple, none, repeated, case, literal wildcards, date boundaries, pagination | Planning: `search_meals_matches_literally_and_case_insensitively`, `search_meals_by_date_range_only`, `search_meals_pages_without_skips_or_repeats`, `search_meals_cursor_survives_inserted_meals`, `search_meals_rejects_foreign_cursors`, `list_weeks_page_walks_every_page_once`. MCP: `search_paginates_through_the_cursor`. |
| 5 | Shopping queries distinguish missing plans, missing days, no lists, empty lists and shared lists | Planning: `shopping_lists_on_monday_and_wednesday`, `shopping_lists_on_days_without_lists`, `shopping_lists_on_keeps_empty_lists_and_categories`. MCP: `shopping_cases_through_mcp`. |
| 6 | MCP and the read API agree; recipe retrieval has every field and the source date; copies do not change history | MCP: `get_week_equals_the_read_api_and_get_meal_has_every_field`. Planning: `get_meal_returns_every_field`, `saved_history_is_independent_of_copies`. |
| 7 | Creating and revising a week preserves ids, order and content; the next web/API read shows it | Planning: `save_creates_a_week_and_round_trips`, `save_revises_with_reordering_drops_and_additions`, `saved_content_is_visible_without_restarting`. MCP: `saved_week_is_visible_without_rebuilding_the_router`. Browser: `week_saved_through_mcp_renders_in_the_browser`. |
| 8 | Invalid saves change nothing; concurrent and external writes are atomic and detected | Planning: `save_rejects_invalid_documents_and_leaves_the_database_unchanged`, `save_detects_stale_and_missing_revisions`, `save_detects_external_edits`, `save_is_atomic_with_concurrent_readers_and_writers`. MCP: `invalid_and_conflicting_saves_change_nothing`. |
| 9 | Read tools cannot write; page and API mutations still return 405; database errors are safe; bad hosts and origins are rejected | MCP: `read_tools_never_write_and_never_touch_the_write_pool`, `pages_and_api_still_reject_mutations`, `database_failures_return_a_safe_error`, `host_and_origin_validation`. Planning: `write_pool_writes_and_read_pool_rejects`, `database_errors_are_safe`. |
| 10 | A recorded MCP client run plans, recalls and shops without database access | MCP: `mcp_client_smoke_test` (run with `--nocapture` to print the negotiated protocol version and client). Last run: rmcp 3.8 client `sous-smoke-client 0.1.0`, protocol `2026-07-28`; the legacy handshake negotiated `2025-11-25`. |

## Feature 7: year archive pages

Backend tests use fixed clocks in `Europe/Berlin` unless stated. `parse_year_path` has the unit test `parses_year_paths`.

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | `/2026` lists exactly the stored weeks of ISO week-year 2026 newest first, including future weeks; `/2026/1` is on `/2026` and `/2020/53` on `/2020` | Backend: `year_page_lists_the_year_newest_first`, `year_title_is_not_zero_padded`, `first_and_last_supported_years`. Browser: `year_page_orders_weeks_and_links_navigate`. |
| 2 | “All weeks” on `/2025/50` links to `/2025` with or without a plan; at `/` it links to the current ISO week-year, `/2026` on December 29, 2025; the invalid week page links to the current year | Backend: `all_weeks_target_year_is_in_the_page_payload`. Browser: `all_weeks_link_targets_the_displayed_year`, `plan_header_range_and_all_weeks_link_by_keyboard`, `invalid_week_page_and_zero_padded_redirect`. |
| 3 | Previous and next year links appear only when the adjacent year has a stored plan | Backend: `year_links_skip_gap_years`, `first_and_last_supported_years`. Browser: `year_header_links_in_order_same_style_one_row_right_aligned`, `year_links_skip_gap_years_and_empty_year_keeps_links`. |
| 4 | Empty-year statuses and messages; with an empty database, “All weeks” from `/` leads to a 200 page | Backend: `year_status_empty_database_and_current_year`, `current_year_follows_the_iso_week_year_in_the_service_timezone`, `year_page_503_when_database_unavailable`. Browser: `empty_database_pages`, `year_links_skip_gap_years_and_empty_year_keeps_links`. |
| 5 | `/weeks`, `/weeks/` and non-year single-segment paths return the plain 404 without server errors | Backend: `non_year_single_segments_are_plain_404`, `year_path_methods`, `unknown_paths_keep_plain_404`. |
| 6 | Year pages and their year links reflect committed changes on refresh | Backend: `year_page_is_fresh`. Browser: `year_link_appears_after_another_connection_commits_adjacent_plan`. |

Year pages are captured at 375 px and 1280 px by `visual_capture_at_375px`/`1280px` (`year`, `empty-year`), and `list_pages_fit_375px` checks that a year page with all three header links does not overflow.

## Feature 8: API routes

Backend tests use the fixed clock of the year archive tests unless stated.

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | `/api/2026/44` returns the week document and `/api/2026/044` redirects to it | Backend: `api_matches_page_bootstrap`, `api_zero_padded_redirects`, `openapi_matches_api`. MCP: `get_week_equals_the_read_api_and_get_meal_has_every_field`. |
| 2 | `/api/weeks/2026/44`, `/api/weeks`, `/api/0000`, `/api/10000` and `/api/nothing` return the JSON `not_found` response | Backend: `api_error_contract`, `api_year_contract`. |
| 3 | `/api/2026` lists the same weeks, in the same order, and the same adjacent years as the `/2026` page, including gap years | Backend: `api_year_matches_year_page_bootstrap`. |
| 4 | Every `/api/<year>` case has its status, content type and stable JSON code, around New Year and with an empty or unavailable database | Backend: `api_year_contract`, `api_year_current_year_around_new_year`, `api_mutations_405_and_no_leaks`. |
| 5 | The OpenAPI document describes both routes and their error codes, has no `/weeks` path, and its examples match the server | Backend: `openapi_matches_api`. |
| 6 | The API reflects committed changes on the next request | Backend: `api_year_is_fresh`, `api_freshness_after_commit`. |

## Feature 9: Docker image and CI

Shutdown tests are in `tests/shutdown.rs`. The local container checks, which record the image size, ran with Podman
on an arm64 host. The amd64 build, pull-request and tag checks are deferred until the repository is published on GitHub.
Each result goes in the manual verification table when it is run.

| # | Criterion | Evidence |
| --- | --- | --- |
| 1 | `docker build .` succeeds from a clean checkout on amd64 and arm64 hosts | Manual: arm64 build (below). Pending: an amd64 build, from the publishing workflow's pull-request run. |
| 2 | The final stage is `FROM scratch`, copies only `/sous`, `/data` and `/tmp`, and the binary is statically linked | The `Dockerfile` final stage is `FROM scratch` with exactly those three `COPY` lines. Manual: `file` on the extracted binary (below). |
| 3 | The container serves `/` and `/api/<year>/<week>` as UID 65532, creates `/data/sous.db`, and keeps its data across a restart | Manual: run, UID and restart check (below). |
| 4 | MCP requests through the published port succeed with the default allowed hosts | Manual: MCP initialize through the published port (below). |
| 5 | `docker run --rm -v sous-data:/data <image> migrate` applies migrations and exits 0 | Manual: migrate-only run (below). |
| 6 | `docker stop` exits with status 0 well before the timeout, and the logs show a graceful shutdown | Backend: `sigterm_shuts_down_gracefully_with_status_0`, `sigint_shuts_down_gracefully_with_status_0`. Manual: graceful stop (below). |
| 7 | The build context excludes local databases, `target/` and `.git` | Manual: build-context listing (below). |
| 8 | On a pull request, the publishing workflow builds both platforms and pushes nothing | Pending: pull-request run, not yet run. |
| 9 | A matching `vX.Y.Z` tag publishes a manifest list under `X.Y.Z`, `X.Y` and `latest`; a mismatched tag fails before building | Pending: test tag push, not yet run. |
| 10 | On a pull request, the test workflow runs all four jobs and passes; a failing test, a formatting difference or a clippy warning fails its job | Pending: pull-request run, not yet run. |

## Manual verification

| Date | Check | Result |
| --- | --- | --- |
| 2026-10-31 | Review of `target/visual/` screenshots at 375 px and 1280 px against `docs/examples/menu.html` | Pass: typography, colors, card and badge styles, strategy themes, empty states and selections match; differences limited to the approved ones (bullet shopping lists, ISO day order, archive pages) |
| 2026-10-31 | Visual check of the running service (`make reset && make run`): all tabs, expanded cards, empty states, archive, current and archived week selection, desktop and narrow mobile | Pass (user) |
| 2026-10-31 | Connect a real MCP client to the running service: `claude mcp add --transport http sous http://127.0.0.1:3000/mcp`, then `claude mcp list` | Pass (user): Claude Code 2.1.309 reports `sous: http://127.0.0.1:3000/mcp (HTTP) - ✔ Connected` |
| 2026-11-01 | With the connected client, plan a new week | Pass (user): Claude Code called `list_weeks` and `search_meals` (date range) to review recent plans, then `save_week` for 2026 week 45 with `expectedRevision: null`; the result returned the committed document (7 days, 2 shopping lists, 3 prep sessions, 4 strategy cards) and a new revision |
| 2026-11-01 | With the connected client, recall "the casserole from three weeks ago" and its steps | Pass (user): Claude Code called `get_context` (today 2026-11-01, Europe/Berlin, week 44), then `get_meal` for the meal found by `search_meals`; it returned Chicken & Broccoli Casserole, Wednesday 2026-10-07 (2026 week 41), with all fields and its 4 stored steps in order |
| 2026-11-01 | With the connected client, fetch today's shopping lists | Pass (user): Claude Code called `get_context`, then `get_shopping_lists` for 2026-11-01 (Europe/Berlin); it returned `planExists: true` and only the list assigned to Sunday, "🛒 Sunday Lunch → Manchego, Apple & Walnut Salad", with its categories and items in stored order |
| 2026-11-01 | Feature 9, check 1: build the image from the working tree on an arm64 host (`podman build`, tagged `sous`) | Pass (user) |
| 2026-11-01 | Feature 9, check 2: extract `/sous` from the image and run `file` on it; record the image size | Pass (user): `ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV), statically linked, … stripped`; image `localhost/sous:latest` is 10.2 MB |
| 2026-11-01 | Feature 9, check 3: run the image with the named volume `sous-data` and `-p 3000:3000` (Podman); request `/` and `/api/2026/44`; read the process user; list `/data` before and after `podman restart` | Pass (user): `/` returned `200 OK` and `/api/2026/44` `404 Not Found` (empty database); the process runs as `65532`; `/data` held `sous.db` (180224 bytes) with its `-shm` and `-wal` files, owned by `65532:65532`, unchanged after the restart |
| 2026-11-01 | Feature 9, check 4: with the container from check 3 and the default allowed hosts, `POST http://localhost:3000/mcp` with an `initialize` request | Pass (user): `200 OK`, `application/json`, protocol `2025-11-25`, `serverInfo` `sous 0.1.0` with the server instructions |
| 2026-11-01 | Feature 9, check 5: `podman run --rm -v sous-data:/data sous migrate` | Pass (user): logged `migrations applied db=/data/sous.db` and exited 0 |
| 2026-11-01 | Feature 9, check 6: `time podman stop sous` on the container from check 3, then its exit code and last log lines | Pass: the stop took 0.37 s (user); exit code `0`; the logs end with `shutting down signal="SIGTERM"` and `stopped` |
| 2026-11-01 | Feature 9, check 7: build a probe image that copies the whole context and lists it, from a working tree that contains `sous.db`, `target/` and `.git` (`podman build`, which applies `.dockerignore`) | Pass (user): the context held only `Cargo.lock`, `Cargo.toml`, `build.rs`, `migrations`, `src` and `web` |
