const { useState, useEffect } = React;

const THEME_KEY = "sous-theme";
const DARK_QUERY = "(prefers-color-scheme: dark)";

// The stored choice: "light", "dark" or "system" (nothing stored, an unknown value or unreadable storage).
function storedTheme() {
  try {
    const value = window.localStorage.getItem(THEME_KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch (e) {
    return "system";
  }
}

// Sets data-theme to the theme in effect; "system" resolves through the device's preference.
function applyTheme(choice) {
  const theme = choice === "system" ? (window.matchMedia(DARK_QUERY).matches ? "dark" : "light") : choice;
  document.documentElement.setAttribute("data-theme", theme);
}

const THEME_OPTIONS = [
  ["light", "Light"],
  ["dark", "Dark"],
  ["system", "System"],
];

// The footer: a link to the household profile (every page but the profile's own), then the theme toggle.
function ThemeToggle({ profileLink = true }) {
  const [choice, setChoice] = useState(storedTheme);
  // While the choice is System, follow the device's preference as it changes.
  useEffect(() => {
    if (choice !== "system") return undefined;
    const query = window.matchMedia(DARK_QUERY);
    const onChange = () => applyTheme("system");
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
  }, [choice]);
  const choose = (next) => {
    try {
      if (next === "system") window.localStorage.removeItem(THEME_KEY);
      else window.localStorage.setItem(THEME_KEY, next);
    } catch (e) {
      // Storage is unavailable: the choice still applies to the open page.
    }
    setChoice(next);
    applyTheme(next);
  };
  return (
    <footer className="footer">
      {profileLink && (
        <a className="footer-link" href="/profile">
          Household
        </a>
      )}
      <div className="theme" role="radiogroup" aria-labelledby="theme-label">
        <span id="theme-label" className="theme-label">
          Theme
        </span>
        {THEME_OPTIONS.map(([value, label]) => (
          <label key={value} className="theme-option">
            <input type="radio" name="theme" value={value} checked={choice === value} onChange={() => choose(value)} />
            <span>{label}</span>
          </label>
        ))}
      </div>
    </footer>
  );
}

// Circle = Sous, square = shopping, triangle = prep. Decorative: the label beside it carries the meaning.
function Shape({ kind }) {
  return <span className={`shape shape-${kind}`} aria-hidden="true"></span>;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAY_NAMES = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

const MEAL_KINDS = ["Breakfast", "Lunch", "Dinner", "Snack"];

// Display a number with at most one decimal place, dropping a trailing ".0".
function fmt(n) {
  return String(Math.round(n * 10) / 10);
}

// "2026-10-26" -> "Oct 26" (parsed manually; no Date, so no timezone shifts).
function formatDay(iso) {
  const [, month, day] = iso.split("-").map(Number);
  return `${MONTHS[month - 1]} ${day}`;
}

function formatDateRange(week) {
  return `${formatDay(week.startDate)} – ${formatDay(week.endDate)}`;
}

// "2026-10-26" -> "Oct 26, 2026".
function formatDayWithYear(iso) {
  return `${formatDay(iso)}, ${Number(iso.split("-")[0])}`;
}

// "Oct 26 – Nov 1, 2026" within one year, "Dec 28, 2026 – Jan 3, 2027" across years.
function formatRangeWithYear(start, end) {
  const startYear = start.split("-")[0];
  const endYear = end.split("-")[0];
  if (startYear === endYear) {
    return `${formatDay(start)} – ${formatDayWithYear(end)}`;
  }
  return `${formatDayWithYear(start)} – ${formatDayWithYear(end)}`;
}

function DaySummary({ meals }) {
  const sum = (key) => meals.reduce((acc, m) => acc + m[key], 0);
  const items = [
    { label: "Calories", value: sum("kcal"), unit: "kcal", cls: "c-kcal" },
    { label: "Protein", value: sum("proteinG"), unit: "g", cls: "c-protein" },
    { label: "Fat", value: sum("fatG"), unit: "g", cls: "c-fat" },
    { label: "Carbs", value: sum("carbsG"), unit: "g", cls: "c-carbs" },
    { label: "Fiber", value: sum("fiberG"), unit: "g", cls: "c-fiber" },
  ];
  return (
    <div className="summary">
      {items.map((item) => (
        <div key={item.label} className="summary-item">
          <div className={`summary-value ${item.cls}`}>
            {fmt(item.value)}
            <span className="summary-unit">{item.unit}</span>
          </div>
          <div className="summary-label">{item.label}</div>
        </div>
      ))}
    </div>
  );
}

function MealCard({ meal }) {
  const [expanded, setExpanded] = useState(false);
  const kind = MEAL_KINDS.includes(meal.type) ? meal.type.toLowerCase() : "snack";
  const detailsId = `meal-${meal.id}`;
  const macros = [
    { label: "P", value: meal.proteinG, cls: "c-protein" },
    { label: "F", value: meal.fatG, cls: "c-fat" },
    { label: "C", value: meal.carbsG, cls: "c-carbs" },
    { label: "Fib", value: meal.fiberG, cls: "c-fiber" },
  ];
  return (
    <div className={`meal meal-${kind}${expanded ? " is-open" : ""}`}>
      <button
        type="button"
        className="meal-toggle"
        aria-expanded={expanded}
        aria-controls={detailsId}
        onClick={() => setExpanded(!expanded)}
      >
        <span className="meal-head">
          <span className="meal-main">
            <span className="meal-meta">
              <span className="meal-type">{meal.type}</span>
              {meal.forTwo && <span className="badge badge-two">For two</span>}
              {meal.assemblyTime && <span className="badge badge-assembly">Assemble {meal.assemblyTime}</span>}
              {meal.cookTime && <span className="badge badge-cook">Cook {meal.cookTime}</span>}
            </span>
            <span className="meal-name">{meal.name}</span>
          </span>
          {meal.kcal > 0 && (
            <span className="meal-kcal">
              {fmt(meal.kcal)} <span className="meal-kcal-unit">kcal</span>
            </span>
          )}
          <span className="meal-indicator" aria-hidden="true">
            {expanded ? "−" : "+"}
          </span>
        </span>
      </button>
      {expanded && (
        <div className="meal-details" id={detailsId}>
          <p className="meal-desc">{meal.description}</p>
          <div className="macros">
            {macros.map((m) => (
              <span key={m.label} className={`macro ${m.cls}`}>
                {m.label} {fmt(m.value)}g
              </span>
            ))}
          </div>
          {meal.steps.length > 0 && (
            <div className="steps-block">
              <div className="steps-title">Instructions</div>
              <ol className="steps">
                {meal.steps.map((step) => (
                  <li key={step.id}>{step.text}</li>
                ))}
              </ol>
            </div>
          )}
          {meal.tip && (
            <div className="tip">
              <span className="tip-label">
                <Shape kind="circle" />
                Tip
              </span>
              <span>{meal.tip}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ShoppingListCard({ list }) {
  const [expanded, setExpanded] = useState(false);
  const bodyId = `shop-${list.id}`;
  return (
    <div className={`shop${expanded ? " is-open" : ""}`}>
      <button
        type="button"
        className="shop-toggle"
        aria-expanded={expanded}
        aria-controls={bodyId}
        onClick={() => setExpanded(!expanded)}
      >
        <span className="shop-head">
          <span className="shop-main">
            <span className="band-label">
              <Shape kind="square" />
              Shopping
            </span>
            <span className="shop-label">{list.label}</span>
            {list.note !== null && <span className="shop-note">{list.note}</span>}
          </span>
          <span className="shop-indicator" aria-hidden="true">
            {expanded ? "−" : "+"}
          </span>
        </span>
      </button>
      {expanded && (
        <div className="shop-body" id={bodyId}>
          {list.categories.length === 0 && <p className="shop-empty">No shopping items.</p>}
          {list.categories.map((cat) => (
            <div key={cat.id} className="shop-cat">
              <div className="shop-cat-name">{cat.name}</div>
              {cat.items.length > 0 && (
                <ul className="shop-items">
                  {cat.items.map((item) => (
                    <li key={item.id}>{item.text}</li>
                  ))}
                </ul>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function PrepCard({ session }) {
  const [expanded, setExpanded] = useState(false);
  const bodyId = `prep-${session.id}`;
  return (
    <div className={`prep${expanded ? " is-open" : ""}`}>
      <button
        type="button"
        className="prep-toggle"
        aria-expanded={expanded}
        aria-controls={bodyId}
        onClick={() => setExpanded(!expanded)}
      >
        <span className="prep-head">
          <span className="prep-main">
            <span className="band-label">
              <Shape kind="triangle" />
              Prep
            </span>
            <span className="prep-title">{session.title}</span>
            {session.covers !== null && <span className="prep-covers">{session.covers}</span>}
          </span>
          <span className="prep-indicator" aria-hidden="true">
            {expanded ? "−" : "+"}
          </span>
        </span>
      </button>
      {expanded && (
        <div className="prep-body" id={bodyId}>
          {session.tasks.map((task) => (
            <div key={task.id} className="prep-task">
              <div className="prep-task-head">
                <span className="prep-task-name">{task.name}</span>
                <span className="prep-task-time">{task.time}</span>
              </div>
              <p className="prep-task-instructions">{task.instructions}</p>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Reminder({ reminder }) {
  return (
    <div className="reminder">
      {reminder.icon !== null && <span className="reminder-icon">{reminder.icon}</span>}
      <div>
        <div className="reminder-title">{reminder.title}</div>
        {reminder.body !== null && <div className="reminder-body">{reminder.body}</div>}
      </div>
    </div>
  );
}

// A step to the previous or next day. A target without a valid date path is shown disabled and is not a link.
function DayStep({ target, label, children }) {
  if (!hasDatePath(target)) {
    return (
      <a className="day-step is-disabled" aria-disabled="true" aria-label={label}>
        {children}
      </a>
    );
  }
  return (
    <a className="day-step" href={datePath(target)} aria-label={label}>
      {children}
    </a>
  );
}

// The header, the day row whose heading toggles the calendar, the calendar, the page content and the footer.
function DayShell({ page, week, heading, subtitle, date, children }) {
  const [open, setOpen] = useState(false);
  const dayNum = dayNumber(date);
  const dayHeading = formatDayHeading(dayNum);
  return (
    <div className="app">
      <Header title={heading} subtitle={subtitle}>
        {/* Today link: the server's today, never the browser's clock; absent on today's own page. */}
        {page.displayedDate !== page.today && (
          <a className="all-weeks today-link" href={"/" + page.today}>
            Today
          </a>
        )}
      </Header>
      <div className="day-row">
        <DayStep target={dayNum - 1} label="Previous day">
          ‹
        </DayStep>
        <h2 className="day-heading">
          <button
            type="button"
            className="day-toggle"
            aria-expanded={open}
            aria-controls="calendar"
            aria-label={`${dayHeading}, calendar`}
            onClick={() => setOpen(!open)}
          >
            {dayHeading}
          </button>
        </h2>
        <DayStep target={dayNum + 1} label="Next day">
          ›
        </DayStep>
      </div>
      <Calendar open={open} page={page} week={week} />
      {children}
      <ThemeToggle />
    </div>
  );
}

function MealPlan({ page }) {
  const data = page.document;
  const day = data.days.find((d) => d.date === page.displayedDate);
  // A session shows on the day of its weekday; a session without tasks has nothing to show.
  const prepSessions = data.prepSessions.filter((s) => s.weekday === day.weekday && s.tasks.length > 0);
  const listsById = new Map(data.shoppingLists.map((l) => [l.id, l]));
  const hasTasks = day.shoppingListIds.length > 0 || prepSessions.length > 0 || day.reminder !== null;
  return (
    <DayShell
      page={page}
      week={data.week}
      heading={`Meal Plan: ${formatDateRange(data.week)}`}
      subtitle={data.week.subtitle}
      date={page.displayedDate}
    >
      {day.meals.length === 0 ? (
        <p className="empty">No meals planned for this day.</p>
      ) : (
        <>
          <DaySummary meals={day.meals} />
          <div className="meals">
            {day.meals.map((meal) => (
              <MealCard key={meal.id} meal={meal} />
            ))}
          </div>
        </>
      )}
      {hasTasks && (
        <div className="tasks">
          {day.shoppingListIds.map((listId) => (
            <ShoppingListCard key={listId} list={listsById.get(listId)} />
          ))}
          {prepSessions.length > 0 && (
            <>
              {data.week.prepIntro !== null && <p className="prep-intro">{data.week.prepIntro}</p>}
              {prepSessions.map((s) => (
                <PrepCard key={s.id} session={s} />
              ))}
            </>
          )}
          {day.reminder !== null && <Reminder reminder={day.reminder} />}
        </div>
      )}
    </DayShell>
  );
}

// The header: the page title, then the subtitle and any links. With neither, the row is not rendered.
function Header({ title, subtitle, children }) {
  return (
    <div className="header">
      <h1 className="title">{title}</h1>
      {(subtitle || children) && (
        <div className="header-meta">
          {subtitle && <p className="subtitle">{subtitle}</p>}
          {children}
        </div>
      )}
    </div>
  );
}

// A page without a day row: the content, then the theme toggle.
function Shell({ children }) {
  return (
    <div className="app">
      {children}
      <ThemeToggle />
    </div>
  );
}

// Proleptic Gregorian dates as day numbers (0 = 1970-01-01), integer arithmetic only: no Date, so the browser's
// timezone does not matter and years 0-99 are not mapped to 1900-1999.
function daysFromCivil(year, month, day) {
  const y = month <= 2 ? year - 1 : year;
  const era = Math.floor(y / 400);
  const yoe = y - era * 400;
  const doy = Math.floor((153 * (month > 2 ? month - 3 : month + 9) + 2) / 5) + day - 1;
  const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
  return era * 146097 + doe - 719468;
}

function civilFromDays(days) {
  const z = days + 719468;
  const era = Math.floor(z / 146097);
  const doe = z - era * 146097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36524) - Math.floor(doe / 146096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const day = doy - Math.floor((153 * mp + 2) / 5) + 1;
  const month = mp < 10 ? mp + 3 : mp - 9;
  return [yoe + era * 400 + (month <= 2 ? 1 : 0), month, day];
}

function dayNumber(iso) {
  const [y, m, d] = iso.split("-").map(Number);
  return daysFromCivil(y, m, d);
}

// 0 = Monday (1970-01-01 was a Thursday).
function weekdayIndex(days) {
  return (((days + 3) % 7) + 7) % 7;
}

const MIN_DAY = daysFromCivil(1, 1, 1);
const MAX_DAY = daysFromCivil(9999, 12, 31);

const MONTH_NAMES = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

function paddedYear(year) {
  return String(year).padStart(4, "0");
}

function twoDigits(n) {
  return String(n).padStart(2, "0");
}

// A day number as "YYYY-MM-DD", the year zero-padded to four digits.
function isoDate(days) {
  const [y, m, d] = civilFromDays(days);
  return `${paddedYear(y)}-${twoDigits(m)}-${twoDigits(d)}`;
}

// The page path of a day, "/2026-11-11".
function datePath(days) {
  return "/" + isoDate(days);
}

// A date path needs a representable ISO week: its Monday from 0001-01-01 and its Sunday up to 9999-12-31.
function hasDatePath(days) {
  const monday = days - weekdayIndex(days);
  return monday >= MIN_DAY && monday + 6 <= MAX_DAY;
}

// "Wednesday, November 11": full names, no year.
function formatDayHeading(days) {
  const [, month, day] = civilFromDays(days);
  return `${WEEKDAY_NAMES[weekdayIndex(days)]}, ${MONTH_NAMES[month - 1]} ${day}`;
}

// The Monday-first ISO week that starts on `monday`: its ISO year and number (the Thursday decides the year).
function isoWeekOf(monday) {
  const [isoYear] = civilFromDays(monday + 3);
  return { isoYear, isoWeek: Math.floor((monday + 3 - daysFromCivil(isoYear, 1, 1)) / 7) + 1 };
}

// The day whose month the calendar opens on: today when no date is displayed, the displayed date on a date page
// (`/YYYY-MM-DD`), otherwise the Thursday of the displayed week.
function initialMonthDay(page, week) {
  if (page.displayedDate === null) return dayNumber(page.today);
  if (/^\/\d{4}-\d{2}-\d{2}$/.test(window.location.pathname)) return dayNumber(page.displayedDate);
  return dayNumber(week.startDate) + 3;
}

function Calendar({ open, page, week }) {
  const { today, plannedWeeks, markedDays, displayedDate } = page;
  const [shown, setShown] = useState(() => {
    const [year, month] = civilFromDays(initialMonthDay(page, week));
    return { year, month };
  });
  const { year, month } = shown;
  const step = (delta) => {
    const index = shown.year * 12 + shown.month - 1 + delta;
    setShown({ year: Math.floor(index / 12), month: (index % 12) + 1 });
  };
  const planned = new Set(plannedWeeks.map((w) => `${w.isoYear}-${w.isoWeek}`));
  const marks = new Map(markedDays.map((d) => [d.date, d]));
  const todayDay = dayNumber(today);
  const first = daysFromCivil(year, month, 1);
  const last = daysFromCivil(month === 12 ? year + 1 : year, month === 12 ? 1 : month + 1, 1) - 1;
  const rows = [];
  for (let monday = first - weekdayIndex(first); monday <= last; monday += 7) {
    rows.push(monday);
  }
  return (
    <section id="calendar" className={open ? "calendar is-open" : "calendar"} aria-label="Calendar">
      <div className="cal-nav">
        <button
          type="button"
          className="cal-step"
          aria-label="Previous month"
          disabled={year === 1 && month === 1}
          onClick={() => step(-1)}
        >
          ‹
        </button>
        <div className="cal-title">
          {MONTH_NAMES[month - 1]} {paddedYear(year)}
        </div>
        <button
          type="button"
          className="cal-step"
          aria-label="Next month"
          disabled={year === 9999 && month === 12}
          onClick={() => step(1)}
        >
          ›
        </button>
      </div>
      <div className="cal-head" aria-hidden="true">
        <span className="cal-wk">Wk</span>
        {["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].map((d) => (
          <span key={d} className="cal-cell">
            {d}
          </span>
        ))}
      </div>
      {rows.map((monday) => {
        const iso = isoWeekOf(monday);
        const isPlanned = planned.has(`${iso.isoYear}-${iso.isoWeek}`);
        // A week that cannot be a path (the last row of December 9999) shows its dates but links none.
        const linked = hasDatePath(monday);
        const cells = [
          <span key="wk" className="cal-wk">
            {isPlanned && <span className="cal-dot" aria-hidden="true"></span>}
            {iso.isoWeek}
          </span>,
        ];
        for (let i = 0; i < 7; i++) {
          const day = monday + i;
          const [y, m, d] = civilFromDays(day);
          const cls = ["cal-cell"];
          if (y !== year || m !== month) cls.push("cal-out");
          if (day === todayDay) cls.push("cal-today");
          if (!linked) {
            cells.push(
              <span key={i} className={cls.join(" ")}>
                {d}
              </span>,
            );
            continue;
          }
          const date = isoDate(day);
          const mark = marks.get(date);
          const names = [];
          if (mark && mark.shopping) names.push("shopping");
          if (mark && mark.prep) names.push("prep");
          const selected = date === displayedDate;
          if (selected) cls.push("cal-selected");
          cells.push(
            <a
              key={i}
              className={cls.join(" ")}
              href={`/${date}`}
              aria-label={[`${WEEKDAY_NAMES[i]}, ${MONTH_NAMES[m - 1]} ${d}, ${paddedYear(y)}`, ...names].join(", ")}
              aria-current={selected ? "page" : undefined}
            >
              {d}
              {names.length > 0 && (
                <span className="cal-marks" aria-hidden="true">
                  {mark.shopping && <span className="cal-mark cal-mark-shopping"></span>}
                  {mark.prep && <span className="cal-mark cal-mark-prep"></span>}
                </span>
              )}
            </a>,
          );
        }
        const cls = ["cal-row"];
        if (isPlanned) cls.push("cal-planned");
        return (
          <div key={monday} className={cls.join(" ")}>
            {cells}
          </div>
        );
      })}
    </section>
  );
}

// Invalid pages: the message and its link, then the calendar, always shown.
function CalendarPage({ page, children }) {
  return (
    <div className="app">
      <Header title="Meal Plan" />
      {children}
      <Calendar open={true} page={page} week={null} />
      <ThemeToggle />
    </div>
  );
}

function Nav({ links }) {
  return (
    <nav className="nav">
      {links.map((l) => (
        <a key={l.href} className="nav-link" href={l.href}>
          {l.label}
        </a>
      ))}
    </nav>
  );
}

const THIS_WEEK = { href: "/", label: "This week" };

// "/2026", zero-padded like week URLs ("/0001").
function yearPath(year) {
  return "/" + String(year).padStart(4, "0");
}

function WeekList({ weeks }) {
  return (
    <ul className="week-list">
      {weeks.map((w) => (
        <li key={w.path}>
          <a className="week-link" href={w.path}>
            <span className="week-link-title">
              Week {w.isoWeek}
              <span className="week-link-year"> · {w.isoYear}</span>
            </span>
            <span className="week-link-range">{formatRangeWithYear(w.startDate, w.endDate)}</span>
            <span className="week-link-arrow" aria-hidden="true">
              ›
            </span>
          </a>
        </li>
      ))}
    </ul>
  );
}

function MissingWeek({ page }) {
  return (
    <DayShell
      page={page}
      week={page.week}
      heading={`Meal Plan: ${formatRangeWithYear(page.week.startDate, page.week.endDate)}`}
      date={page.displayedDate}
    >
      <p className="empty">No meal plan for this week yet.</p>
    </DayShell>
  );
}

function YearPage({ page }) {
  return (
    <Shell>
      <Header title={`Meal Plan: ${page.year}`}>
        {page.previousYear !== null && (
          <a className="all-weeks" href={yearPath(page.previousYear)}>
            ← {page.previousYear}
          </a>
        )}
        <a className="all-weeks" href={THIS_WEEK.href}>
          {THIS_WEEK.label}
        </a>
        {page.nextYear !== null && (
          <a className="all-weeks" href={yearPath(page.nextYear)}>
            {page.nextYear} →
          </a>
        )}
      </Header>
      {page.weeks.length === 0 ? (
        <p className="empty">No meal plans available yet.</p>
      ) : (
        <WeekList weeks={page.weeks} />
      )}
    </Shell>
  );
}

function InvalidWeek({ page }) {
  return (
    <CalendarPage page={page}>
      <p className="empty">Invalid week.</p>
      <Nav links={[THIS_WEEK]} />
    </CalendarPage>
  );
}

function InvalidDate({ page }) {
  return (
    <CalendarPage page={page}>
      <p className="empty">Invalid date.</p>
      <Nav links={[THIS_WEEK]} />
    </CalendarPage>
  );
}

function Unavailable() {
  return (
    <Shell>
      <Header title="Meal Plan" />
      <p className="empty">Meal plans are temporarily unavailable.</p>
      <Nav links={[{ href: "/", label: "Try again" }]} />
    </Shell>
  );
}

const SECTION_HINTS = {
  household: "People, portions and dietary restrictions.",
  food: "Likes, dislikes and cuisines.",
  kitchen: "Equipment, storage and limitations.",
  planning: "Cooking time, shopping, budget, leftovers and batch prep.",
  other: "Anything else worth knowing.",
};

// One profile section: a labelled text field, a Save button and one live region for the save status, an error
// or the conflict notice. `saved` is the last saved text and revision; `onSaved` reports a new one to the page.
function ProfileSection({ section, saved, onSaved }) {
  const { key, title } = section;
  const [draft, setDraft] = useState(saved.text);
  const [busy, setBusy] = useState(false);
  // What the live region shows: null, "saved", "error" or { text, revision } (a conflict with the saved section).
  const [status, setStatus] = useState(null);
  const fieldId = `profile-${key}`;
  const hintId = `${fieldId}-hint`;
  const conflict = status !== null && typeof status === "object" ? status : null;

  const save = async (revision) => {
    setBusy(true);
    try {
      const response = await fetch("/api/profile/" + key, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: draft, expectedRevision: revision }),
      });
      if (response.status === 200) {
        const next = await response.json();
        onSaved(key, next);
        setStatus("saved");
      } else if (response.status === 409) {
        const { current } = await response.json();
        setStatus({ text: current.text, revision: current.revision });
      } else {
        setStatus("error");
      }
    } catch (e) {
      setStatus("error");
    } finally {
      setBusy(false);
    }
  };
  const useSaved = () => {
    onSaved(key, { text: conflict.text, revision: conflict.revision });
    setDraft(conflict.text);
    setStatus(null);
  };

  return (
    <section className="field">
      <label className="field-title" htmlFor={fieldId}>
        {title}
      </label>
      <p className="field-hint" id={hintId}>
        {SECTION_HINTS[key]}
      </p>
      <textarea
        id={fieldId}
        className="text-field"
        rows={4}
        value={draft}
        aria-describedby={hintId}
        onChange={(e) => {
          setDraft(e.target.value);
          setStatus(conflict);
        }}
      />
      <div className="field-actions">
        <button type="button" className="button" disabled={busy || draft === saved.text} onClick={() => save(saved.revision)}>
          Save
        </button>
        <div role="status" className={status === "saved" || status === null ? "field-status" : "field-status is-notice"}>
          {status === "saved" && "Saved."}
          {status === "error" && <p className="notice">Couldn't save. Try again.</p>}
          {conflict !== null && (
            <div className="notice">
              <p>This section was changed elsewhere.</p>
              {conflict.text !== "" ? (
                <p className="notice-text">{conflict.text}</p>
              ) : (
                <p className="notice-text notice-empty">The saved version is empty.</p>
              )}
              <div className="notice-actions">
                <button type="button" className="button" disabled={busy} onClick={() => save(conflict.revision)}>
                  Keep mine
                </button>
                <button type="button" className="button" disabled={busy} onClick={useSaved}>
                  Use saved
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    </section>
  );
}

function ProfilePage({ page }) {
  const [saved, setSaved] = useState(() => Object.fromEntries(page.sections.map((s) => [s.key, s])));
  const onSaved = (key, next) => setSaved((all) => ({ ...all, [key]: { ...all[key], text: next.text, revision: next.revision } }));
  const allEmpty = page.sections.every((s) => saved[s.key].text === "");
  return (
    <div className="app">
      <Header title="Household">
        <a className="all-weeks" href={THIS_WEEK.href}>
          {THIS_WEEK.label}
        </a>
      </Header>
      {allEmpty && <p className="empty">No household details yet.</p>}
      <div className="fields">
        {page.sections.map((s) => (
          <ProfileSection key={s.key} section={s} saved={saved[s.key]} onSaved={onSaved} />
        ))}
      </div>
      <ThemeToggle profileLink={false} />
    </div>
  );
}

function App({ page }) {
  switch (page.kind) {
    case "profile":
      return <ProfilePage page={page} />;
    case "week":
      return <MealPlan page={page} />;
    case "missing":
      return <MissingWeek page={page} />;
    case "year":
      return <YearPage page={page} />;
    case "invalid":
      return <InvalidWeek page={page} />;
    case "invalidDate":
      return <InvalidDate page={page} />;
    default:
      return <Unavailable />;
  }
}

const page = JSON.parse(document.getElementById("sous-page").textContent);
ReactDOM.createRoot(document.getElementById("root")).render(<App page={page} />);
