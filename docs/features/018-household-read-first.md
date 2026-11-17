# 18. Read-first household page

## Outcome

The household page reads like a profile, not a form. People come to check what Sous knows and now and then fix one fact, so each section shows its saved text, with an Edit button that opens the field in place. The first section is called "People", so "Household" is only the page title.

Depends on: [17. Household profile](017-household-profile.md).

## Scope

This changes the web page and one section title. It supersedes these parts of feature 17's "Web page" contract:

- the empty state "No household details yet." is removed;
- each section is read-first instead of an always-open text field;
- Save is hidden while a conflict notice is open;
- the first section's title and hint change.

Storage, section keys, revisions, conflict detection, routes, statuses, request checks and error codes don't change. No migration.

## Behaviour

### Section title

- The `household` section's title becomes "People", everywhere titles are returned: `get_context`, `GET /api/profile`, the `PUT /api/profile/{key}` and `update_profile_section` results, the 409 `current` section and the profile page's initial JSON. The key stays `household`.
- Its hint becomes "Who eats, portions and dietary restrictions." The other four titles and hints don't change.
- The MCP server instructions name the sections by their titles: "(people, food preferences, kitchen, planning habits, other context)". The `get_context` description lists keys and doesn't change.

### Reading

- On load, every section is in read mode.
- A section in read mode shows a row with its title as an `<h2>` and an "Edit" button at the right end of the row. Below the row is the saved text, rendered as escaped text with its line breaks kept, never as HTML or Markdown.
- An empty section shows its hint in place of the text, so it says what to write. A section with text shows no hint.
- The Edit button's visible text is "Edit". Its accessible name is "Edit" followed by the section title, such as "Edit People", so the five buttons can be told apart.
- The page has no empty state. With nothing saved, each section shows its hint, so the page reads the same before and after.

### Editing

- Edit opens the field in place of the title row and text: the text field with the title as its `<label>` and the hint under it (`aria-describedby`, as now), filled with the saved text. Below it are "Save" and "Cancel", in that order. The field takes focus.
- Save is enabled only while the draft differs from the saved text, and every button in the section is disabled while a save is in flight, as now. Cancel is enabled except while a save is in flight.
- Cancel drops the draft and closes the field, with no prompt.
- A successful save (200) closes the field and shows the saved text. The live region then shows "Saved." under the text until the section's next Edit.
- Any other failure keeps the field open with the draft, Save and Cancel, and shows "Couldn't save. Try again." in the live region, as now.
- Several sections may be open at once. Each saves on its own, and edits to different sections never conflict, as now.
- There is no prompt when the user leaves the page with an open field.

### Conflict

- A 409 keeps the field open with the draft, and the notice opens as now: "This section was changed elsewhere.", then `current.text`, or "The saved version is empty." in `ink-muted` when it's empty, then "Keep mine" and "Use saved".
- While the notice is open, Save and Cancel are hidden. Keep mine and Use saved are the only ways forward. Typing in the field leaves the notice open.
- Keep mine saves the draft against `current.revision`. On 200 it closes the notice and the field, and shows "Saved.". On another 409 the notice stays open with the newer text and revision, as now.
- Use saved takes `current` as the saved text and revision, closes the notice and the field, and shows `current.text` in read mode with no status.

### Focus and the live region

- Closing the field by Save, Cancel, Keep mine or Use saved puts focus on that section's Edit button.
- Each section keeps one polite live region (`role="status"`), mounted in both modes as the section's last child, so "Saved." is announced when a save closes the field. When empty, it adds no space.

## Layout and styles

- Read mode: the title row is a flex row, title on the left in `title` `ink`, Edit at its right end, vertically centred. The text is `body` `ink` with `white-space: pre-wrap` and `overflow-wrap: anywhere`, `space-2` below the row. An empty section's hint is `small` `ink-muted`, also `space-2` below.
- Edit mode: the text field as now. Save and Cancel sit in a row `space-3` below it, `space-3` apart.
- The live region sits `space-3` below what comes before it: "Saved." in `small` `ink-muted`, or the notice in `body`. Empty, it takes no space.
- Sections stay `space-5` apart. Edit, Save and Cancel use the existing Button. Nothing new needs a colour outside the tokens.
- Nothing overflows at 320 px, also with a long unbroken word in the text.

## Documentation

- [Design system](../design-system.md), written by one owner:
  - Voice: drop "No household details yet." from the household page's words.
  - Components: add **Profile section**, describing read and edit mode as above. In **Text field**, the label is the section title only in edit mode. In **Button**, list `Edit`, `Save`, `Cancel`, `Keep mine` and `Use saved` as its words. In **Notice**, it sits in the section's live region below the field, and Save and Cancel are hidden while it's open. "Saved." shows under the text after the field closes.
  - Page order: the profile page is the header, one profile section per section, then the theme toggle footer without the Footer link. There's no empty state.
- [OpenAPI document](../openapi.json): the `Section.title` description lists "People, Food preferences, Kitchen, Planning habits or Other context", and the example's `household` title becomes "People".
- [MCP guide](../mcp.md) and the [technical design](../technical-design.md): update any section title or page description that says otherwise. Regenerate the [tool list](../mcp-tools.json) if a description changed.
- [Feature 17](017-household-profile.md) stays as written. This spec supersedes the parts listed under Scope.

## Acceptance criteria

1. On load, every section shows its title, an Edit button and its saved text with line breaks kept, or its hint when it's empty. There's no "No household details yet." on the page, whatever is saved.
2. Edit opens the field with the saved text and focuses it. Save is disabled until the draft changes. Cancel drops the draft, closes the field and focuses Edit. A successful Save closes the field, shows the new text with "Saved.", and focuses Edit. The next Edit clears "Saved.".
3. A failed save keeps the field open with the draft, Save, Cancel and "Couldn't save. Try again.".
4. During a conflict the field stays open, Save and Cancel are hidden, and only Keep mine and Use saved are shown. Keep mine on 200 and Use saved each close the field. Keep mine on a second 409 keeps the notice with the newer text.
5. The `household` section's title is "People" in `get_context`, the HTTP API and the page, with the hint "Who eats, portions and dietary restrictions.". Its key, its stored text and its revision are unchanged.
6. The page follows the design system in light and in dark, with each Edit button named after its section. Nothing overflows at 320 px.
7. Routes, statuses, keys, revisions and conflict handling are unchanged. The existing tests pass after being updated for the read-first page and the new title.

## Verification

- Browser tests:
  - Read mode on load: titles, Edit buttons with their accessible names, text with line breaks, and the hint on empty sections. No empty state when all five are empty.
  - Edit, type, Cancel: the text is unchanged, the field is closed, focus is on Edit. Edit, type, Save: the new text shows with "Saved.", focus is on Edit, and the next Edit clears the status.
  - A failed save keeps the field, Save and Cancel open.
  - Update the existing conflict tests, including the third-writer test, to open the field with Edit first, and assert that Save and Cancel are hidden while the notice is open and that Use saved and a successful Keep mine close the field.
  - Screenshots at 375 and 1280 px, light and dark, in read mode, edit mode and during a conflict.
- Backend and MCP tests: the `household` section's title is "People" in `get_context`, `GET /api/profile`, the PUT and `update_profile_section` results and the 409 body. Revisions computed before the change still match, because the revision hashes the key and text, not the title.
