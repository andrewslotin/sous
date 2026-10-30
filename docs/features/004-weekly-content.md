# 4. Prep, strategy, and reminders

## Outcome

Sous represents all weekly content in the example using database records, including prep sessions, strategy cards, and day-specific reminders.

Depends on: [3. Shopping lists](003-shopping-lists.md).

## Scope

- Add migrations for `prep_sessions`, `prep_tasks`, `strategy_cards`, and `strategy_items`.
- Add nullable `weeks.prep_intro` and the day reminder title/body/icon columns. Existing data remains valid without these fields.
- Extend the shared week document and query with the new content.
- Add Prep Sessions and Strategy tabs with the reference appearance and transient selection state.
- Render the prep introduction and ordered expandable session cards, including coverage text, task names, time labels, and instructions.
- Render ordered strategy cards with stored titles, optional icons, ordered bullet points, and predefined green, blue, purple, or yellow themes.
- Render optional day reminders after the meals using the reference reminder styling. Do not hardcode Saturday, a particular assistant, or example-specific wording.

## Content rules

Store preparation times as display text. Do not interpret durations or turn instructions into executable markup. Strategy themes are a constrained enum, not arbitrary CSS.

A reminder exists when its title is non-null; its body and icon are optional. Without a title, omit the reminder. For empty prep/strategy collections, retain the tabs and show “No prep sessions planned.” or “No strategy notes for this week.” respectively.

The title's date range derives from the selected week. Subtitle, prep introduction, strategy text, and reminders come from the database. UI labels remain application text.

## Acceptance criteria

1. Existing databases upgrade without losing previously populated meals or shopping lists.
2. A complete fixture renders all three tabs with the reference typography, spacing, colors, and expandable cards.
3. Prep sessions/tasks and strategy cards/items follow stored order, including after reordering records.
4. All four strategy themes render correctly; unsupported themes are rejected by the database.
5. Optional text and missing reminders render without blank placeholder blocks. Reminders can appear on any day.
6. Empty prep and strategy tabs show the specified messages.
7. No sample-specific subtitle, prep introduction, strategy bullet, or reminder remains hardcoded.
8. Committed changes appear on refresh; database text is escaped, and tabs/expanders remain keyboard accessible.

## Verification

Test migration preservation, constraints, and ordered query assembly using populated databases from earlier iterations. Verify all tabs and optional/empty states in mobile and desktop browsers, including expanded prep cards.
