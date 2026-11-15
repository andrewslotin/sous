# Sous design system

Sous shows one household's weekly meal plan: a day at a time, on a phone propped up in the kitchen. The look comes from Kandinsky's Bauhaus years: a cream canvas, thin black lines, and the three primaries each tied to one shape. Blue circle, red square, yellow triangle. Every shape and every primary on the page means something, so a cook can tell shopping from prep from across the counter.

This document is the visual reference for the web interface. It replaces the [example page](examples/menu.html), which is kept as history.

## Principles

- **The day is the page.** Every view centres on one day: what to cook, what to buy, what to prep. Put nothing above the day heading except the page title and subtitle.
- **Shape and colour carry meaning, together.** A **blue circle** is Sous itself (navigation and the selected day), a **red square** is shopping, a **yellow triangle** is prep. A shape always comes with its word, never as decoration, and with its colour everywhere except the calendar, where the marks are `ink` so a month stays calm.
- **Black line, flat planes.** Rectangles are square-cornered and flat. Edges are hairlines of `line`; an open card's edge, or an open meal's top rule, turns `ink`, like one of Kandinsky's black lines. Only the day's tasks get a box. Meals are open rows on the page, split by hairlines. No shadows, no gradients, no rounded cards.
- **Read at arm's length.** Body copy is 16px Jost (`body`) with generous leading. Never set recipe text below 14px.

## Content

- Write for one cook in their own kitchen. Use "you" sparingly and never "we". Sentence case everywhere except `label` text, which CSS uppercases.
- Keep the app's words few and literal: "No meals planned for this day.", "No meal plan for this week yet.", "Invalid date.", "Meal plans are temporarily unavailable." End each with a full stop and don't add jokes.
- Show dates as `Wednesday, November 11` (day heading), `Nov 9 – Nov 15` (ranges, en dash with spaces) and `Week 46 · 2026` (archive). Show quantities as `520 kcal` and `P 32g`, with one decimal at most and a trailing `.0` dropped.
- Meal names, recipes, shopping items and notes are household data. Render them as given: don't title-case, truncate or rewrite them.
- **No emoji in the interface's own text.** A `label` and its shape do the job (`■ SHOPPING`, `▲ PREP`, `● TIP`). Household data, such as a reminder's own `icon` or a list label, may contain emoji, because the household wrote it.

## Themes

There are two themes, light and dark. The reader picks Light, Dark or System in the footer of every page. System is the default and follows the device's colour-scheme preference, including when it changes while the page is open. The choice is kept on the device and applied before the first paint, so the page never shows the wrong theme first.

## Colour

- **Grounds.** `paper` is the page: raw canvas in light, a deep blue-black in dark. `surface` is cards, and `surface-sunken` holds things inside a card (the nutrition strip, prep tasks, the reminder) and is the hover fill.
- **Ink.** Text is `ink`, secondary text `ink-muted`. `ink-faint` is for disabled steppers, out-of-month dates and weekday heads only (3:1), never reading text.
- **Blue (Sous).** The selected calendar day is a solid `blue` disc with `on-blue` text, and the only colour in the calendar. The "For two" badge and the tip box use `blue` on `blue-soft`.
- **Red (shopping).** A shopping card's band is solid `red` with `on-red` text and a square before `SHOPPING`. Category names inside are `red`.
- **Yellow (prep).** A prep card's band is solid `yellow` with `on-yellow` text and a triangle before `PREP`. Task times are `ink` chips on `yellow-soft`. Yellow only fills; it's never text on a light ground.
- **Meals stay plain.** Meal rows sit straight on `paper` and carry no primary. Macro figures use `macro-protein`, `macro-fat`, `macro-carbs` and `macro-fiber`, always beside their letter (P, F, C, Fib). Calories stay `ink`.
- **The calendar is ink.** Its shopping squares and prep triangles are `ink`, and the planned-week circle is `ink-muted`. Only the selected day is blue, and its marks turn `on-blue`.
- Both themes pass: every text token meets 4.5:1 on the grounds its note names, and marks, the today ring and stepper rings meet 3:1.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `paper` | `#f1e9d8` | `#0f1220` | Page background behind every view. |
| `surface` | `#fbf7ee` | `#181c2c` | Cards: shopping lists, prep sessions, the week list; the stepper fill. |
| `surface-sunken` | `#e9e0cc` | `#0a0c16` | Recessed areas: the nutrition strip, a prep task, the reminder; hover on calendar days and week rows. |
| `line` | `#dcd1ba` | `#2b3044` | Hairline borders and dividers. Decorative only: never the sole edge of a control. |
| `line-strong` | `#7a705c` | `#737a96` | Borders that carry meaning: disabled steppers, dashed reminder and empty-state borders, header link underlines. 3:1 on paper and surface. |
| `ink` | `#17171c` | `#f2ede1` | Primary text and the black line: stepper rings, the today ring, an open card's edge, calendar marks. |
| `ink-muted` | `#55514a` | `#b0b2bf` | Secondary text: subtitles, ranges, meal-type labels, descriptions, header links, unchosen theme options. |
| `ink-faint` | `#857c69` | `#7f8499` | Out-of-month dates, disabled steppers, weekday heads. 3:1 only. |
| `blue` | `#1d3f9e` | `#86a0ff` | Sous: the selected calendar day, the "For two" badge, the tip label and its circle. |
| `blue-soft` | `#dde3f4` | `#1c2550` | Behind blue text: the "For two" badge and the tip box. |
| `on-blue` | `#fbf7ee` | `#0a0c16` | Text, marks and the today ring on the selected day. |
| `red` | `#b8321b` | `#ff7a5c` | Shopping: the shopping band, category names. |
| `on-red` | `#fbf7ee` | `#0a0c16` | Text and the square on the shopping band. |
| `yellow` | `#f2c12e` | `#f5c84a` | Prep: the prep band. A fill only. |
| `on-yellow` | `#17171c` | `#0a0c16` | Text and the triangle on the prep band. |
| `yellow-soft` | `#f8e8b0` | `#3b310f` | Task-time chips inside a prep card, with `ink` text. |
| `macro-protein` | `#a8430e` | `#ff9b62` | Protein figures. |
| `macro-fat` | `#755700` | `#e6c05c` | Fat figures. |
| `macro-carbs` | `#35652a` | `#9fd27f` | Carbohydrate figures. |
| `macro-fiber` | `#683d91` | `#c8a3f2` | Fibre figures. |
| `focus` | `ink` | `ink` | Keyboard focus outline. |

## Type

- **Jost** (`sans`) sets every word: a geometric sans in the Futura line. **JetBrains Mono** (`mono`) sets numbers so they line up. There is no serif. Both are self-hosted woff2 files: Jost at 400, 400 italic and 600, JetBrains Mono at 400, 600 and 700.
- `sans`: `"Jost", "Futura", "Century Gothic", system-ui, sans-serif`. `mono`: `"JetBrains Mono", ui-monospace, Menlo, monospace`.

| Style | Family | Size / line height | Weight | Other | Use |
| --- | --- | --- | --- | --- | --- |
| `display` | sans | 28/34px | 600 | -0.01em | The page title, once per page. |
| `heading` | sans | 20/26px | 600 | | The day heading. |
| `title` | sans | 17/23px | 600 | | Card titles: meal names, shopping-list and prep-session titles, week links; the calendar month. |
| `body` | sans | 16/24px | 400 | | Recipe steps, prep instructions, shopping items, empty states. |
| `body-italic` | sans | 16/24px | 400 | italic | A meal's description. |
| `small` | sans | 14/20px | 400 | | Subtitles, date ranges, notes, prep "covers" lines, header links, the theme toggle. |
| `label` | sans | 11/14px | 600 | 0.12em, uppercase | Meal type, nutrition labels, "Instructions", "Tip", band labels, weekday heads, "Theme". |
| `figure` | mono | 22/26px | 600 | -0.02em | Day totals in the nutrition strip. |
| `figure-sm` | mono | 13/18px | 600 | | A meal's kcal, macros, task times, badges (11px), calendar dates, week numbers. |

## Shapes, space, layout

- **Radii are shapes.** `radius-square` (0) for every rectangle: cards, bands, the summary strip, the week list, prep tasks, the tip. `radius-soft` (2px) for badges and chips only. `radius-circle` (50%) for circles only: day steppers, calendar days and the planned-week mark. A circle always means Sous's own navigation.
- **Shapes.** The circle, square and triangle are drawn in CSS at 10px (8px in the tip label, about 7px as calendar marks), and hidden from assistive technology because a word always sits beside them.
- **Lines.** Steppers are 40px circles with a 1.5px `ink` ring on `surface`, filled `ink` on hover. Today in the calendar is a 1.5px `ink` ring; when it's also selected, an `on-blue` ring sits inset inside the blue disc so today stays marked.
- Spacing runs `space-1` 4, `space-2` 8, `space-3` 12, `space-4` 16, `space-5` 24 and `space-6` 32 (px). Cards pad `space-4`. Cards in a stack sit `space-3` apart, and page sections `space-5` apart. Meal rows pad `space-4` top and bottom and nothing at the sides, and stack with no gap.
- The page is one centred column of at most `column-width` (560px) on every width, padded `space-4` on phones and `space-6` at the top on wide screens. Nothing sits beside it.
- **The calendar is hidden by default** on day pages. The day heading is its toggle, marked as a link by a dashed underline: tap it and the month opens as a section of the column, right below the heading, straight on `paper` between two `line` hairlines. Tap it again to close it; picking a day opens that day's page with the calendar closed.
- **Focus:** every link, button and radio shows a solid 2px `focus` outline at a 2px offset, outside the element, so it sits on `paper` or `surface` and never on a band. Nothing clips it.

## Components

- **Header:** the page title in `display` `ink`, then a row with the subtitle and any links in `small` `ink-muted`. Links are underlined in `line-strong`. With no subtitle and no links, the row isn't rendered. Day pages have no links. Year pages show `← 2025`, `This week`, `2027 →`.
- **Day navigation:** the day heading in `heading`, centred between two steppers (`‹ ›` in `mono`, labelled "Previous day" and "Next day"). A disabled stepper has a `line-strong` ring, `ink-faint` chevron and no fill. The heading is a button that shows and hides the calendar: a dashed `ink-muted` underline that turns `ink` on hover and while the calendar is open.
- **Calendar:** no fill; the month sits on `paper` between two `line` hairlines. The month in `title` between borderless steppers, weekday heads in `label` `ink-faint`, a week-number column with a small `ink-muted` circle for planned weeks, and days as 40px circles with the date in `figure-sm`. Selected, today and marks as under Colour; out-of-month dates are `ink-faint`. Shopping is a small square and prep a small triangle.
- **Nutrition summary:** a five-column square `surface-sunken` strip with `line` dividers. Each column has a `figure` value, its unit stacked under it, and a `label` under that. Calories are `ink`; the macros take their macro colours.
- **Meal card:** an open row on `paper` with a `line` hairline above it; one more hairline closes the list. The toggle shows the meal type as a `label` in `ink-muted` with the badges, the meal name in `title`, the kcal in `figure-sm` on the right (`520 kcal`, the unit `ink-muted`) and a `+`/`−` indicator in `mono`. Open, the top hairline turns `ink`, and it shows the description in `body-italic` `ink-muted`, the macros in `figure-sm`, an `INSTRUCTIONS` `label` over numbered steps in `body`, and the tip: a square `blue-soft` box headed by a blue circle and a `TIP` label in `blue`.
- **Badges:** 11px `mono` 600, uppercase, `radius-soft`. "For two" is `blue` on `blue-soft`. "Assemble 10 min" and "Cook 25 min" are `ink-muted` on `surface-sunken`.
- **Shopping list:** a square `surface` card with a `line` border, turning `ink` when open. Its header band is solid `red` with `on-red` text: a square and `SHOPPING` in `label`, the list label in `title` and its note in `small`, and the `+`/`−` indicator. Open, category names in `label` `red` over items in `body` `ink`, separated by `line` rules.
- **Prep session:** like the shopping list with a solid `yellow` band, `on-yellow` text, a triangle and `PREP`. Open, each task is a square `surface-sunken` block with its name in `body` semibold, its time as an `ink` chip on `yellow-soft`, and its instructions in `body` `ink-muted`. The prep introduction sits above the cards in `body` `ink-muted`.
- **Reminder:** a square `surface-sunken` block with a dashed `line-strong` border, its icon, the title in `body` semibold and the body in `small` `ink-muted`.
- **Week list:** one square `surface` panel with `line` dividers; each row has `Week 46` in `title`, `· 2026` in `ink-muted`, its range in `small` `ink-muted` and a `›` in `mono` on the right.
- **Empty state:** `body` `ink-muted` text, centred in a dashed `line-strong` box with no fill. Links that follow it are `small` `ink-muted` and underlined, like header links.
- **Theme toggle:** a footer at the end of every page, with a `line` hairline above it and `space-5` padding. A `THEME` `label` and the options Light, Dark and System in `small`, centred. Unchosen options are `ink-muted` and turn `ink` on hover; the chosen one is `ink` with a solid underline. It's a radio group of native radios.

Build the day page in this order: header, day navigation, the calendar while it's open, the nutrition summary, a meal card for each meal, then one stack of the day's tasks (a shopping list for each list due that day, the prep introduction and prep sessions, and the reminder), and the theme toggle footer. If the day has no meals, an empty state replaces the summary and meals, and the tasks stay below it.

## Iconography

Sous has no icon set and no logo. Set the name in `display` type. The only glyphs are the three shapes, the stepper chevrons `‹ ›`, the week list's `›` and the expand indicators `+ −`, set in `mono`.
