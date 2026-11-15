# 14. Redesign

## Outcome

Every page uses the new [design system](../design-system.md), after Kandinsky's Bauhaus years. It has a cream canvas and thin black lines. Jost sets words and JetBrains Mono sets numbers. Each primary goes with one shape: a blue circle for Sous and the selected day, a red square for shopping and a yellow triangle for prep. The interface's own text has no emoji. The page has a light and a dark theme. The reader picks Light, Dark or System in a footer, and System follows the device.

Depends on: [13. Day view](013-day-view.md). This feature replaces the [example page](../examples/menu.html) as the visual reference. That page stays as history.

## Scope

- Restyle every page: the day view, the missing-plan, invalid week, invalid date, year and unavailable pages.
- Add the theme toggle and keep its choice on the device.
- Change the layout and markup only where this spec says so.
- Routes, statuses, data, the API and the MCP tools do not change. Expansion, keyboard access, the calendar's links and marks, the day links and their disabled states stay as they are, except as listed under Layout.
- No Node. The only new assets are the Jost fonts, which are vendored in `web/fonts` with their licence. Remove the Source Serif 4 files, which nothing uses any more.

## Theme

- `<html>` always carries `data-theme="light"` or `data-theme="dark"`: the theme in effect, not the choice.
- `web/styles.css` defines every colour token from the design system on `:root` with its light value, plus `color-scheme: light`. It redefines the colour tokens with their dark values on `:root[data-theme="dark"]`, plus `color-scheme: dark`. It also defines the spacing, radius, layout and font family tokens as custom properties. Component rules use only the tokens, never raw colour values.
- The choice is stored in `localStorage` under `sous-theme`, as `light` or `dark`. Choosing System removes the key. A missing, unknown or unreadable value means System.
- A small inline script in the HTML shell's `<head>`, before the stylesheet, reads the choice and sets `data-theme`. System resolves through `matchMedia("(prefers-color-scheme: dark)")`. The page never paints in the wrong theme first. If storage throws, the script falls back to System, and the toggle still works for the open page.
- The app reads the same choice for the toggle's initial value. Choosing an option stores it and sets `data-theme` at once, with no reload. While the choice is System, the app follows changes to the device's preference while the page is open.

## Layout

- Every page is one centred column, at most 560px, on every width. The calendar sidebar at 900px and wider is gone, and so is the header's "Calendar" button.
- **Day pages:** the day heading is a button that shows and hides the calendar. It has `aria-expanded`, `aria-controls="calendar"`, and an `aria-label` of the heading plus ", calendar". It has a dashed underline. The calendar is hidden by default. Open, it sits right below the day row and above the content. Picking a date opens that date's page with the calendar closed, as a new page load already does.
- **Day page order:** header, day row, the calendar (when open), the nutrition summary and the meal cards, then the shopping lists, the prep introduction and prep sessions, and the reminder. If the day has no meals, "No meals planned for this day." replaces the summary and meals, and the shopping lists, prep and reminder stay below it.
- **Missing-plan page:** it gets the same day row for its displayed date, with working previous and next day links and the heading that toggles the calendar. "No meal plan for this week yet." follows.
- **Invalid week and invalid date pages:** no day row and no toggle. The message and its "This week" link come first, and the calendar is always shown below them. It opens on today's month, as now.
- **Year and unavailable pages:** unchanged apart from the styles.
- **Every page** ends with the theme toggle footer.

## Styles

Follow the design system's Components section. In particular:

- The page is `paper`. Shopping and prep cards, the week list and steppers are `surface`. No shadows, no gradients, no rounded rectangles: rectangles have `radius-square`, badges and chips `radius-soft`, and only steppers, calendar days and the planned-week mark are circles.
- Meals are open rows on `paper` with `line` hairlines between them and below the last. An open meal's top hairline turns `ink`. Meal rows have no fill, side borders or meal-type colour.
- Shopping and prep cards have a solid band: `red` with `on-red` text and a square for shopping, `yellow` with `on-yellow` text and a triangle for prep. An open card's border turns `ink`.
- The nutrition summary is a five-column `surface-sunken` strip with `line` dividers and the unit stacked under each figure.
- The calendar has no fill and sits between two `line` hairlines. The selected date is a `blue` disc with `on-blue` text. Today has an inset 1.5px `ink` ring, which is `on-blue` when today is selected. The shopping mark is a small square and the prep mark a small triangle, both `ink`, and `on-blue` on the selected date. The planned-week mark is a small `ink-muted` circle.
- The reminder and the empty-state messages have a dashed `line-strong` border.
- Every link, button and radio shows a solid 2px `focus` outline at a 2px offset under keyboard focus. Nothing clips it.

## Markup changes

- Meal cards:
  - No meal-type icons. The meal type is a label.
  - The badges read "For two", "Assemble {assemblyTime}" and "Cook {cookTime}", with no emoji. CSS uppercases them.
  - The kcal reads `{kcal} kcal`, with the unit in its own element.
  - The toggle ends with a `+`/`−` indicator, hidden from assistive technology.
  - The tip has no 💡. It is a box with a circle and a `Tip` label above the tip text.
- Shopping cards: the band shows a square and a `Shopping` label above the list label and note.
- Prep cards: the title has no 🔪. The band shows a triangle and a `Prep` label above the title and covers line.
- The shapes are empty elements drawn by CSS and hidden from assistive technology.
- The year page's week links get a decorative `›` on the right, hidden from assistive technology.
- A reminder's own icon stays: it is household data.
- Labels are uppercased by CSS, so the text and accessible names stay in sentence case. "For two" replaces "FOR TWO" as UI text.
- Theme toggle: a `<footer>` holding a radio group (`role="radiogroup"`, labelled by a visible "Theme" label) of three native radios named `theme`, with the values `light`, `dark` and `system` and the labels Light, Dark and System.

## Documentation

- Replace the [design system](../design-system.md) document with the Kandinsky system: the principles, the token tables for both themes, type, shapes, space, layout and components.
- Technical design:
  - Make the design system the visual reference in the purpose and in "Interface preservation".
  - Replace the rule to keep the example page's width, gradients, colours and fonts with the design system's rules.
  - Remove "the existing meal-type colors and icons" from "Retain".
  - Under "Intentional changes", add the redesign, the theme toggle and the removed emoji. Replace the calendar sidebar with the calendar toggled from the day heading. Give the day view's new order.
  - Replace the body font sentence with the two self-hosted families, Jost and JetBrains Mono.
  - Note that the theme choice is the one piece of UI state kept on the device.
  - Point verification criterion 1 at the design system.
- Docs index: point the visual reference at the design system.
- README: describe the look and the theme toggle.
- Features 1–13 stay as history.

## Acceptance criteria

1. Every page matches the design system in light and in dark.
2. `web/styles.css` defines the colour tokens once per theme, on `:root` and `:root[data-theme="dark"]`. No component rule contains a raw colour value. Jost and JetBrains Mono are self-hosted, and the Source Serif 4 files are gone.
3. Every page ends with the theme toggle. System is the default and follows the device's preference, including a change while the page is open. Light and Dark pin the theme and survive a reload. The first paint after a reload is already in the chosen theme. System removes the stored choice.
4. The interface's own text has no emoji. Household data, such as a reminder's icon or a list label, is rendered as given. Badges, the tip and the shopping and prep bands carry their labels.
5. Every page is one column, at most 560px, on every width, with no sidebar and no header "Calendar" button. On day and missing-plan pages the calendar is hidden until the day heading opens it, and the heading closes it again. On invalid week and invalid date pages the calendar is always shown, below the message.
6. The day page shows the summary and meals before the shopping lists, prep and reminder. The missing-plan page has the day row for its date.
7. Text meets 4.5:1, and marks, the today ring and stepper rings meet 3:1, against their backgrounds in both themes.
8. Every link, button and radio shows the focus outline under keyboard focus. Cards still expand with keyboard and pointer input, and the calendar toggle works with both.
9. Routes, statuses, data and API responses are unchanged. The existing behaviour tests pass, changed only where this spec changes the layout.

## Verification

- Update the browser tests (`cargo test --test browser -- --ignored`) that check old colours, widths, the sidebar, the "Calendar" button, emoji or the day page's order, so they check the new design instead.
- Check token values in both themes, and that System follows an emulated `prefers-color-scheme` change.
- Check that a stored choice survives a reload and that `data-theme` is set before the app script runs.
- Check that there is no emoji outside household data. Check the badge and band texts, the 560px column, the calendar toggle and its ARIA state, the invalid pages' calendar, and the calendar's selected, today and mark styles.
- Capture screenshots in light and in dark:
  - A day with shopping, prep, an expanded meal and a reminder, at 375 px (calendar closed and open) and at 1280 px.
  - The year page, the missing-plan page and the invalid date page, at 375 px.
