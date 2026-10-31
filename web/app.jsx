const { useState } = React;

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const TABS = [
  { id: "plan", label: "Daily Plan" },
  { id: "prep", label: "Prep Sessions" },
  { id: "strategy", label: "Strategy" },
];

// Fixed whitelist: the database theme is mapped to a class here, never used as one directly.
const STRATEGY_THEMES = {
  green: "strategy-green",
  blue: "strategy-blue",
  purple: "strategy-purple",
  yellow: "strategy-yellow",
};

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

function StrategyCard({ card }) {
  const theme = STRATEGY_THEMES[card.theme];
  return (
    <div className={theme ? `strategy ${theme}` : "strategy"}>
      <div className="strategy-title">{card.icon !== null ? `${card.icon} ${card.title}` : card.title}</div>
      {card.items.length > 0 && (
        <ul className="strategy-items">
          {card.items.map((item) => (
            <li key={item.id}>{item.text}</li>
          ))}
        </ul>
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

function MealPlan({ data, selectedWeekday }) {
  const [activeTab, setActiveTab] = useState("plan");
  const [selectedDay, setSelectedDay] = useState(selectedWeekday - 1);
  const day = data.days[selectedDay];
  // A session without tasks has nothing to show, so it is not displayed.
  const prepSessions = data.prepSessions.filter((s) => s.tasks.length > 0);
  const listsById =new Map(data.shoppingLists.map((l) => [l.id, l]));
  return (
    <div className="app">
      <div className="header">
        <h1 className="title">Meal Plan: {formatDateRange(data.week)}</h1>
        <div className="header-meta">
          {data.week.subtitle && <p className="subtitle">{data.week.subtitle}</p>}
          <a className="all-weeks" href={yearPath(data.week.isoYear)}>
            All weeks
          </a>
        </div>
      </div>
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
      {activeTab === "strategy" &&
        (data.strategyCards.length === 0 ? (
          <p className="empty">No strategy notes for this week.</p>
        ) : (
          <div className="strategy-list">
            {data.strategyCards.map((c) => (
              <StrategyCard key={c.id} card={c} />
            ))}
          </div>
        ))}
    </div>
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

function PageTitle({ children, link }) {
  return (
    <>
      <h1 className="title">{children}</h1>
      {link && (
        <div className="header-meta">
          <a className="all-weeks" href={link.href}>
            {link.label}
          </a>
        </div>
      )}
    </>
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

function allWeeks(year) {
  return { href: yearPath(year), label: "All weeks" };
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
    <Shell
      title={<PageTitle link={allWeeks(page.week.isoYear)}>Meal Plan: {formatRangeWithYear(page.week.startDate, page.week.endDate)}</PageTitle>}
    >
      <p className="empty">No meal plan for this week yet.</p>
    </Shell>
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
    <Shell title={<PageTitle>Meal Plan</PageTitle>}>
      <p className="empty">Invalid week.</p>
      <Nav links={[THIS_WEEK, allWeeks(page.currentYear)]} />
    </Shell>
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
      return <MealPlan data={page.document} selectedWeekday={page.selectedWeekday} />;
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
