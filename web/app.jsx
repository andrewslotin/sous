const { useState } = React;

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

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

function MealPlan({ data }) {
  const [selectedDay, setSelectedDay] = useState(0);
  const day = data.days[selectedDay];
  return (
    <div className="app">
      <div className="header">
        <h1 className="title">Meal Plan: {formatDateRange(data.week)}</h1>
        {data.week.subtitle && <p className="subtitle">{data.week.subtitle}</p>}
      </div>
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
    </div>
  );
}

const data = JSON.parse(document.getElementById("sous-week").textContent);
ReactDOM.createRoot(document.getElementById("root")).render(<MealPlan data={data} />);
