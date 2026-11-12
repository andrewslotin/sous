const { useState } = React;

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const TABS = [
  { id: "plan", label: "Daily Plan" },
  { id: "prep", label: "Prep Sessions" },
];

const MEAL_ICONS = { Breakfast: "☀️", Lunch: "🥗", Dinner: "🍽️", Snack: "🥜" };

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
  const kind = MEAL_ICONS[meal.type] ? meal.type.toLowerCase() : "snack";
  const icon = MEAL_ICONS[meal.type] || MEAL_ICONS.Snack;
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
        <div className="meal-head">
          <div className="meal-main">
            <div className="meal-meta">
              <span className="meal-icon">{icon}</span>
              <span className="meal-type">{meal.type}</span>
              {meal.forTwo && <span className="badge badge-two">FOR TWO</span>}
              {meal.assemblyTime && <span className="badge badge-assembly">⚡ {meal.assemblyTime}</span>}
              {meal.cookTime && <span className="badge badge-cook">🍳 {meal.cookTime}</span>}
            </div>
            <div className="meal-name">{meal.name}</div>
          </div>
          {meal.kcal > 0 && <div className="meal-kcal">{fmt(meal.kcal)}</div>}
        </div>
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
          {meal.tip && <div className="tip">💡 {meal.tip}</div>}
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
            <span className="prep-title">🔪 {session.title}</span>
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

function MealPlan({ page }) {
  const data = page.document;
  const [activeTab, setActiveTab] = useState("plan");
  const [selectedDay, setSelectedDay] = useState(page.selectedWeekday - 1);
  const day = data.days[selectedDay];
  // A session without tasks has nothing to show, so it is not displayed.
  const prepSessions = data.prepSessions.filter((s) => s.tasks.length > 0);
  const listsById =new Map(data.shoppingLists.map((l) => [l.id, l]));
  return (
    <CalendarShell
      page={page}
      week={data.week}
      heading={`Meal Plan: ${formatDateRange(data.week)}`}
      subtitle={data.week.subtitle}
    >
      <div className="tabs">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            className="tab-button"
            aria-pressed={activeTab === tab.id}
            onClick={() => setActiveTab(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      {activeTab === "plan" && (
        <>
          <div className="days">
            {data.days.map((d, i) => (
              <button
                key={d.date}
                type="button"
                className="day-button"
                aria-pressed={selectedDay === i}
                onClick={() => setSelectedDay(i)}
              >
                {WEEKDAYS[d.weekday - 1]}
              </button>
            ))}
          </div>
          {day.shoppingListIds.map((listId) => (
            <ShoppingListCard key={`${day.date}-${listId}`} list={listsById.get(listId)} />
          ))}
          {day.meals.length === 0 ? (
            <p className="empty">No meals planned for this day.</p>
          ) : (
            <>
              <DaySummary meals={day.meals} />
              {day.meals.map((meal) => (
                <MealCard key={meal.id} meal={meal} />
              ))}
            </>
          )}
          {day.reminder !== null && <Reminder reminder={day.reminder} />}
        </>
      )}
      {activeTab === "prep" && (
        <>
          {data.week.prepIntro !== null && <p className="prep-intro">{data.week.prepIntro}</p>}
          {prepSessions.length === 0 ? (
            <p className="empty">No prep sessions planned.</p>
          ) : (
            prepSessions.map((s) => <PrepCard key={s.id} session={s} />)
          )}
        </>
      )}
    </CalendarShell>
  );
}

function Shell({ title, children }) {
  return (
    <div className="app">
      <div className="header">{title}</div>
      {children}
    </div>
  );
}

function PageTitle({ children }) {
  return <h1 className="title">{children}</h1>;
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

// The Monday-first ISO week that starts on `monday`: its ISO year and number (the Thursday decides the year).
function isoWeekOf(monday) {
  const [isoYear] = civilFromDays(monday + 3);
  return { isoYear, isoWeek: Math.floor((monday + 3 - daysFromCivil(isoYear, 1, 1)) / 7) + 1 };
}

// "November 9–15, 2026", "October 26–November 1, 2026", "December 29, 2025–January 4, 2026".
function formatWeekRange(monday) {
  const [y1, m1, d1] = civilFromDays(monday);
  const [y2, m2, d2] = civilFromDays(monday + 6);
  const start = `${MONTH_NAMES[m1 - 1]} ${d1}`;
  const end = `${MONTH_NAMES[m2 - 1]} ${d2}, ${paddedYear(y2)}`;
  if (y1 !== y2) {
    return `${start}, ${paddedYear(y1)}–${end}`;
  }
  return m1 === m2 ? `${start}–${d2}, ${paddedYear(y2)}` : `${start}–${end}`;
}

function Calendar({ open, today, plannedWeeks, week }) {
  const [shown, setShown] = useState(() => {
    // The month of the displayed week's Thursday, or of today when no week is displayed.
    const [year, month] = civilFromDays(week ? dayNumber(week.startDate) + 3 : dayNumber(today));
    return { year, month };
  });
  const { year, month } = shown;
  const step = (delta) => {
    const index = shown.year * 12 + shown.month - 1 + delta;
    setShown({ year: Math.floor(index / 12), month: (index % 12) + 1 });
  };
  const planned = new Set(plannedWeeks.map((w) => `${w.isoYear}-${w.isoWeek}`));
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
        const isDisplayed = week && week.isoYear === iso.isoYear && week.isoWeek === iso.isoWeek;
        const cells = [
          <span key="wk" className="cal-wk">
            {isPlanned && <span className="cal-dot" aria-hidden="true"></span>}
            {iso.isoWeek}
          </span>,
        ];
        for (let i = 0; i < 7; i++) {
          const [y, m, d] = civilFromDays(monday + i);
          const cls = ["cal-cell"];
          if (y !== year || m !== month) cls.push("cal-out");
          if (monday + i === todayDay) cls.push("cal-today");
          cells.push(
            <span key={i} className={cls.join(" ")}>
              {d}
            </span>,
          );
        }
        const cls = ["cal-row"];
        if (isPlanned) cls.push("cal-planned");
        // A week that cannot be a path (the last row of December 9999) is shown but is not a link.
        if (monday < MIN_DAY || monday + 6 > MAX_DAY) {
          return (
            <div key={monday} className={cls.join(" ")}>
              {cells}
            </div>
          );
        }
        return (
          <a
            key={monday}
            className={cls.join(" ")}
            href={`/${paddedYear(iso.isoYear)}/${iso.isoWeek}`}
            aria-label={`Week ${iso.isoWeek}, ${formatWeekRange(monday)}${isPlanned ? ", planned" : ""}`}
            aria-current={isDisplayed ? "page" : undefined}
          >
            {cells}
          </a>
        );
      })}
    </section>
  );
}

// The header with the "Calendar" toggle (narrow screens), the calendar, then the page content.
function CalendarShell({ page, week, heading, subtitle, children }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="app has-calendar">
      <div className="header">
        <PageTitle>{heading}</PageTitle>
        <div className="header-meta">
          {subtitle && <p className="subtitle">{subtitle}</p>}
          <button
            type="button"
            className="all-weeks calendar-toggle"
            aria-expanded={open}
            aria-controls="calendar"
            onClick={() => setOpen(!open)}
          >
            Calendar
          </button>
        </div>
      </div>
      <Calendar open={open} today={page.today} plannedWeeks={page.plannedWeeks} week={week} />
      {children}
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
              Week {w.isoWeek} · {w.isoYear}
            </span>
            <span className="week-link-range">{formatRangeWithYear(w.startDate, w.endDate)}</span>
          </a>
        </li>
      ))}
    </ul>
  );
}

function MissingWeek({ page }) {
  return (
    <CalendarShell
      page={page}
      week={page.week}
      heading={`Meal Plan: ${formatRangeWithYear(page.week.startDate, page.week.endDate)}`}
    >
      <p className="empty">No meal plan for this week yet.</p>
    </CalendarShell>
  );
}

function YearPage({ page }) {
  return (
    <Shell
      title={
        <>
          <h1 className="title">Meal Plan: {page.year}</h1>
          <div className="header-meta">
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
          </div>
        </>
      }
    >
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
    <CalendarShell page={page} week={null} heading="Meal Plan">
      <p className="empty">Invalid week.</p>
      <Nav links={[THIS_WEEK]} />
    </CalendarShell>
  );
}

function Unavailable() {
  return (
    <Shell title={<PageTitle>Meal Plan</PageTitle>}>
      <p className="empty">Meal plans are temporarily unavailable.</p>
      <Nav links={[{ href: "/", label: "Try again" }]} />
    </Shell>
  );
}

function App({ page }) {
  switch (page.kind) {
    case "week":
      return <MealPlan page={page} />;
    case "missing":
      return <MissingWeek page={page} />;
    case "year":
      return <YearPage page={page} />;
    case "invalid":
      return <InvalidWeek page={page} />;
    default:
      return <Unavailable />;
  }
}

const page = JSON.parse(document.getElementById("sous-page").textContent);
ReactDOM.createRoot(document.getElementById("root")).render(<App page={page} />);
