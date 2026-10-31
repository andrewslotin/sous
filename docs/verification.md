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
| 4 | Missing current plans, missing explicit weeks, empty archives, missing days and unavailable databases produce their messages and statuses | Backend: `root_missing_current_week_is_200_with_summary`, `explicit_missing_week_is_404_without_weeks`, `weeks_archive_empty_database`, `http_503_when_database_unavailable`, `api_error_contract`. Browser: `missing_week_pages_link_to_archive_only`, `empty_database_pages`, `day_switching_empty_states_and_restore`, `empty_prep_and_strategy_show_messages`, `database_failure_shows_unavailable_not_missing`. |
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

## Manual verification

| Date | Check | Result |
| --- | --- | --- |
| 2026-10-31 | Review of `target/visual/` screenshots at 375 px and 1280 px against `docs/examples/menu.html` | Pass: typography, colors, card and badge styles, strategy themes, empty states and selections match; differences limited to the approved ones (bullet shopping lists, ISO day order, archive pages) |
| 2026-10-31 | Visual check of the running service (`make reset && make run`): all tabs, expanded cards, empty states, archive, current and archived week selection, desktop and narrow mobile | Pass (user) |
