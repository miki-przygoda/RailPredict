//! Query Explorer — a guided, constrained "fill-in-the-gaps" data explorer.
//!
//! The admin composes a query from whitelisted controls (subject, filters, group-by,
//! metric) and the server runs a **safe parameterized** query. There is NO raw SQL:
//! every SQL fragment is chosen by a `match` on an enum (the only interpolation is of
//! these fixed fragments, never user text) and every user value is `push_bind`ed via
//! `sqlx::QueryBuilder`. Always a `LIMIT` + (where applicable) the delay sanity filter
//! + a bounded time window.
//!
//! Three subjects share the same filter / group / result machinery:
//!   - `observations`  — `delay_history ⋈ services` (recorded delays)
//!   - `predictions`   — finalised `prediction_outcomes ⋈ services` (accuracy)
//!   - `cancellations` — `cancellations ⋈ services` (cancelled services)

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{FromRow, QueryBuilder};

use super::Db;

// ---------------------------------------------------------------------------
// Subject
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Subject {
    #[default]
    Observations,
    Predictions,
    Cancellations,
}

impl Subject {
    pub fn parse(s: &str) -> Self {
        match s {
            "predictions" => Self::Predictions,
            "cancellations" => Self::Cancellations,
            _ => Self::Observations,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Observations => "observations",
            Self::Predictions => "predictions",
            Self::Cancellations => "cancellations",
        }
    }
    /// `FROM` table + alias.
    fn from(self) -> &'static str {
        match self {
            Self::Observations => "delay_history d",
            Self::Predictions => "prediction_outcomes p",
            Self::Cancellations => "cancellations c",
        }
    }
    /// Time column for the window / date-range filters and the "day" grouping.
    fn time_col(self) -> &'static str {
        match self {
            Self::Observations => "d.recorded_at",
            Self::Predictions => "p.finalised_at",
            Self::Cancellations => "c.recorded_at",
        }
    }
    fn origin_col(self) -> &'static str {
        match self {
            Self::Observations => "d.origin_crs",
            Self::Predictions => "p.origin_crs",
            Self::Cancellations => "c.origin_crs",
        }
    }
    /// Hour-of-day expression (a column where present, else extracted).
    fn hour_expr(self) -> &'static str {
        match self {
            Self::Observations => "d.departure_hour",
            Self::Predictions => "EXTRACT(HOUR FROM p.scheduled_departure)::int",
            Self::Cancellations => "c.departure_hour",
        }
    }
    /// Weekday expression, normalised to 0 = Monday .. 6 = Sunday.
    fn weekday_expr(self) -> &'static str {
        match self {
            Self::Observations => "d.weekday",
            Self::Predictions => "(EXTRACT(ISODOW FROM p.scheduled_departure)::int - 1)",
            Self::Cancellations => "c.weekday",
        }
    }
    /// An extra always-on WHERE clause (e.g. predictions must be finalised), if any.
    fn base_where(self) -> Option<&'static str> {
        match self {
            Self::Predictions => Some("p.finalised_at IS NOT NULL"),
            _ => None,
        }
    }
    /// The delay sanity filter for this subject, if it has a delay column.
    fn sanity(self) -> Option<&'static str> {
        match self {
            Self::Observations => Some("d.delay_mins BETWEEN -120 AND 600"),
            Self::Predictions => Some("p.final_delay_mins BETWEEN -120 AND 600"),
            Self::Cancellations => None,
        }
    }
    /// The metric chosen when the requested one isn't valid for this subject.
    fn default_metric(self) -> Metric {
        match self {
            Self::Observations => Metric::List,
            _ => Metric::Count,
        }
    }
    fn allows(self, m: Metric) -> bool {
        match self {
            Self::Observations => matches!(
                m,
                Metric::List | Metric::Count | Metric::AvgDelay | Metric::OnTimePct | Metric::P50 | Metric::P90
            ),
            Self::Predictions => matches!(m, Metric::Count | Metric::Mae | Metric::AvgConfidence),
            Self::Cancellations => matches!(m, Metric::Count),
        }
    }
}

// ---------------------------------------------------------------------------
// Group-by
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    None,
    Operator,
    Origin,
    Destination,
    Route,
    Hour,
    Weekday,
    Day,
}

impl GroupBy {
    pub fn parse(s: &str) -> Self {
        match s {
            "operator" => Self::Operator,
            "origin" => Self::Origin,
            "destination" => Self::Destination,
            "route" => Self::Route,
            "hour" => Self::Hour,
            "weekday" => Self::Weekday,
            "day" => Self::Day,
            _ => Self::None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Operator => "operator",
            Self::Origin => "origin",
            Self::Destination => "destination",
            Self::Route => "route",
            Self::Hour => "hour",
            Self::Weekday => "weekday",
            Self::Day => "day",
        }
    }
    fn header(self) -> &'static str {
        match self {
            Self::None => "Group",
            Self::Operator => "Operator",
            Self::Origin => "Origin",
            Self::Destination => "Destination",
            Self::Route => "Route",
            Self::Hour => "Hour",
            Self::Weekday => "Weekday",
            Self::Day => "Day",
        }
    }
    fn needs_services(self) -> bool {
        matches!(self, Self::Operator | Self::Destination | Self::Route)
    }
    /// The SELECT label expression — built from fixed subject column expressions
    /// (never user text).
    fn label_expr(self, subj: Subject) -> String {
        let weekday_names = "(ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])";
        match self {
            Self::None => "'All'".to_string(),
            Self::Operator => "COALESCE(s.toc, 'Unknown')".to_string(),
            Self::Origin => subj.origin_col().to_string(),
            Self::Destination => "COALESCE(s.destination_crs, '?')".to_string(),
            Self::Route => format!("{} || ' \u{2192} ' || COALESCE(s.destination_crs, '?')", subj.origin_col()),
            Self::Hour => format!("LPAD(({})::text, 2, '0') || ':00'", subj.hour_expr()),
            Self::Weekday => format!("{}[({}) + 1]", weekday_names, subj.weekday_expr()),
            Self::Day => format!("to_char({} AT TIME ZONE 'Europe/London', 'YYYY-MM-DD')", subj.time_col()),
        }
    }
    /// The GROUP BY clause, or `None` for a single overall aggregate.
    fn group_clause(self, subj: Subject) -> Option<String> {
        match self {
            Self::None => None,
            Self::Operator => Some("s.toc".to_string()),
            Self::Origin => Some(subj.origin_col().to_string()),
            Self::Destination => Some("s.destination_crs".to_string()),
            Self::Route => Some(format!("{}, s.destination_crs", subj.origin_col())),
            Self::Hour => Some(subj.hour_expr().to_string()),
            Self::Weekday => Some(subj.weekday_expr().to_string()),
            Self::Day => Some(format!("({} AT TIME ZONE 'Europe/London')::date", subj.time_col())),
        }
    }
}

// ---------------------------------------------------------------------------
// Metric
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    List,
    Count,
    AvgDelay,
    OnTimePct,
    P50,
    P90,
    Mae,
    AvgConfidence,
}

impl Metric {
    pub fn parse(s: &str) -> Self {
        match s {
            "count" => Self::Count,
            "avg_delay" => Self::AvgDelay,
            "on_time_pct" => Self::OnTimePct,
            "p50" => Self::P50,
            "p90" => Self::P90,
            "mae" => Self::Mae,
            "avg_confidence" => Self::AvgConfidence,
            _ => Self::List,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Count => "count",
            Self::AvgDelay => "avg_delay",
            Self::OnTimePct => "on_time_pct",
            Self::P50 => "p50",
            Self::P90 => "p90",
            Self::Mae => "mae",
            Self::AvgConfidence => "avg_confidence",
        }
    }
    fn value_expr(self) -> &'static str {
        match self {
            Self::Count => "COUNT(*)::float8",
            Self::AvgDelay => "AVG(d.delay_mins)::float8",
            Self::OnTimePct => "(AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8",
            Self::P50 => "(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY d.delay_mins))::float8",
            Self::P90 => "(PERCENTILE_CONT(0.9) WITHIN GROUP (ORDER BY d.delay_mins))::float8",
            Self::Mae => "AVG(ABS(p.final_delay_mins - p.predicted_delay_mins))::float8",
            Self::AvgConfidence => "AVG(p.prediction_confidence)::float8",
            Self::List => "0::float8",
        }
    }
    fn header(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::AvgDelay => "Avg delay (min)",
            Self::OnTimePct => "On-time %",
            Self::P50 => "Median delay (min)",
            Self::P90 => "p90 delay (min)",
            Self::Mae => "MAE (min)",
            Self::AvgConfidence => "Avg confidence",
            Self::List => "",
        }
    }
}

// ---------------------------------------------------------------------------
// Spec + validation
// ---------------------------------------------------------------------------

const MAX_LIMIT: i64 = 500;
const DEFAULT_LIMIT: i64 = 200;
const DEFAULT_WINDOW_HOURS: i32 = 168; // 7 days

/// Raw (untrusted) inputs, before validation.
#[derive(Debug, Default)]
pub struct RawExplore<'a> {
    pub subject: Option<&'a str>,
    pub window: Option<&'a str>,
    pub from_date: Option<&'a str>,
    pub to_date: Option<&'a str>,
    pub from_hour: Option<i16>,
    pub to_hour: Option<i16>,
    pub weekdays: Option<&'a str>,
    pub operator: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub destination: Option<&'a str>,
    pub min_delay: Option<i32>,
    pub group: Option<&'a str>,
    pub metric: Option<&'a str>,
    pub limit: Option<i64>,
}

/// A validated explore query — every field range-checked / whitelisted.
#[derive(Debug, Clone)]
pub struct ExploreSpec {
    pub subject: Subject,
    pub window_hours: i32,
    pub from_date: Option<NaiveDate>,
    pub to_date: Option<NaiveDate>,
    pub from_hour: Option<i16>,
    pub to_hour: Option<i16>,
    pub weekdays: Vec<i16>,
    pub operator: Option<String>,
    pub origin: Option<String>,
    pub destination: Option<String>,
    pub min_delay: Option<i32>,
    pub group_by: GroupBy,
    pub metric: Metric,
    pub limit: i64,
}

impl ExploreSpec {
    /// Validate raw query-string values into a safe spec. Out-of-range / unparseable
    /// values are clamped or dropped — never an error, never raw SQL.
    pub fn from_raw(raw: RawExplore) -> Self {
        let subject = raw.subject.map(Subject::parse).unwrap_or_default();
        let window_hours = match raw.window {
            Some("24h") => 24,
            Some("7d") => 168,
            Some("30d") => 720,
            Some("all") => 24 * 365 * 100,
            _ => DEFAULT_WINDOW_HOURS,
        };
        let clamp_hour = |h: i16| h.clamp(0, 23);
        let weekdays: Vec<i16> = raw
            .weekdays
            .unwrap_or("")
            .split(',')
            .filter_map(|t| t.trim().parse::<i16>().ok())
            .filter(|d| (0..=6).contains(d))
            .collect();
        let crs = |c: Option<&str>| -> Option<String> {
            c.map(|s| s.trim().to_uppercase())
                .filter(|s| s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic()))
        };
        let date = |d: Option<&str>| d.and_then(|s| s.trim().parse::<NaiveDate>().ok());

        let metric = raw.metric.map(Metric::parse).unwrap_or_default();
        let metric = if subject.allows(metric) { metric } else { subject.default_metric() };

        Self {
            subject,
            window_hours,
            from_date: date(raw.from_date),
            to_date: date(raw.to_date),
            from_hour: raw.from_hour.map(clamp_hour),
            to_hour: raw.to_hour.map(clamp_hour),
            weekdays,
            operator: raw.operator.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            origin: crs(raw.origin),
            destination: crs(raw.destination),
            min_delay: raw.min_delay,
            group_by: raw.group.map(GroupBy::parse).unwrap_or_default(),
            metric,
            limit: raw.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        }
    }

    fn needs_services(&self) -> bool {
        self.group_by.needs_services() || self.operator.is_some() || self.destination.is_some()
    }

    /// Push the WHERE clauses shared by every subject (window, date range, sanity, and
    /// each active filter) — all as bound parameters or fixed fragments.
    fn push_filters(&self, qb: &mut QueryBuilder<'_, sqlx::Postgres>) {
        let subj = self.subject;
        qb.push(" WHERE ");
        qb.push(subj.time_col());
        qb.push(" > NOW() - ");
        qb.push_bind(self.window_hours);
        qb.push("::INT * INTERVAL '1 hour'");
        if let Some(extra) = subj.base_where() {
            qb.push(" AND ");
            qb.push(extra);
        }
        if let Some(sanity) = subj.sanity() {
            qb.push(" AND ");
            qb.push(sanity);
        }
        if let Some(d) = self.from_date {
            qb.push(format!(" AND ({})::date >= ", subj.time_col()));
            qb.push_bind(d);
        }
        if let Some(d) = self.to_date {
            qb.push(format!(" AND ({})::date <= ", subj.time_col()));
            qb.push_bind(d);
        }
        if let Some(h) = self.from_hour {
            qb.push(format!(" AND ({}) >= ", subj.hour_expr()));
            qb.push_bind(h as i32);
        }
        if let Some(h) = self.to_hour {
            qb.push(format!(" AND ({}) <= ", subj.hour_expr()));
            qb.push_bind(h as i32);
        }
        if !self.weekdays.is_empty() {
            qb.push(format!(" AND ({}) = ANY(", subj.weekday_expr()));
            qb.push_bind(self.weekdays.iter().map(|w| *w as i32).collect::<Vec<i32>>());
            qb.push(")");
        }
        if let Some(op) = &self.operator {
            qb.push(" AND s.toc = ");
            qb.push_bind(op.clone());
        }
        if let Some(o) = &self.origin {
            qb.push(format!(" AND {} = ", subj.origin_col()));
            qb.push_bind(o.clone());
        }
        if let Some(dst) = &self.destination {
            qb.push(" AND s.destination_crs = ");
            qb.push_bind(dst.clone());
        }
        // min_delay only applies to the observations subject (it has delay_mins).
        if subj == Subject::Observations
            && let Some(md) = self.min_delay
        {
            qb.push(" AND d.delay_mins >= ");
            qb.push_bind(md);
        }
    }
}

// ---------------------------------------------------------------------------
// Result + runners
// ---------------------------------------------------------------------------

/// Generic tabular result the frontend renders as a table (and can export as CSV).
#[derive(Debug, Clone)]
pub struct ExploreResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

impl ExploreResult {
    /// RFC-4180-ish CSV (quote cells containing `,`, `"`, or newlines).
    pub fn to_csv(&self) -> String {
        let esc = |s: &str| -> String {
            if s.contains([',', '"', '\n']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.to_string()
            }
        };
        let mut out = String::new();
        out.push_str(&self.columns.iter().map(|c| esc(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
        for row in &self.rows {
            out.push_str(&row.iter().map(|c| esc(c)).collect::<Vec<_>>().join(","));
            out.push('\n');
        }
        out
    }
}

#[derive(FromRow)]
struct GroupedRow {
    label: Option<String>,
    value: Option<f64>,
    n: i64,
}

#[derive(FromRow)]
struct ObservationRow {
    uid: String,
    origin_crs: String,
    toc: Option<String>,
    weekday: i16,
    departure_hour: i16,
    delay_mins: i32,
    recorded_at: DateTime<Utc>,
}

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Run a validated explore query. Always parameterized + capped; never raw SQL.
pub async fn run_explore(db: &Db, spec: &ExploreSpec) -> sqlx::Result<ExploreResult> {
    if spec.subject == Subject::Observations && spec.metric == Metric::List {
        run_observation_list(db, spec).await
    } else {
        run_grouped(db, spec).await
    }
}

async fn run_grouped(db: &Db, spec: &ExploreSpec) -> sqlx::Result<ExploreResult> {
    let subj = spec.subject;
    let metric = if subj.allows(spec.metric) && spec.metric != Metric::List {
        spec.metric
    } else {
        Metric::Count
    };

    let mut qb = QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    qb.push(spec.group_by.label_expr(subj));
    qb.push(" AS label, ");
    qb.push(metric.value_expr());
    qb.push(" AS value, COUNT(*) AS n FROM ");
    qb.push(subj.from());
    if spec.needs_services() {
        qb.push(format!(" LEFT JOIN services s ON s.uid = {}.uid", subj_alias(subj)));
    }
    spec.push_filters(&mut qb);
    if let Some(g) = spec.group_by.group_clause(subj) {
        qb.push(" GROUP BY ");
        qb.push(g);
    }
    qb.push(" ORDER BY n DESC LIMIT ");
    qb.push_bind(spec.limit);

    let rows = qb.build_query_as::<GroupedRow>().fetch_all(db).await?;
    let truncated = rows.len() as i64 >= spec.limit;

    let value_fmt = |v: Option<f64>| match (metric, v) {
        (Metric::Count, Some(x)) => format!("{}", x as i64),
        (_, Some(x)) => format!("{x:.2}"),
        (_, None) => "—".to_string(),
    };
    let cells = rows
        .into_iter()
        .map(|r| vec![r.label.unwrap_or_else(|| "—".into()), value_fmt(r.value), r.n.to_string()])
        .collect();

    Ok(ExploreResult {
        columns: vec![
            spec.group_by.header().to_string(),
            metric.header().to_string(),
            "Records".to_string(),
        ],
        rows: cells,
        truncated,
    })
}

async fn run_observation_list(db: &Db, spec: &ExploreSpec) -> sqlx::Result<ExploreResult> {
    let mut qb = QueryBuilder::<sqlx::Postgres>::new(
        "SELECT d.uid AS uid, d.origin_crs AS origin_crs, s.toc AS toc, d.weekday AS weekday, \
         d.departure_hour AS departure_hour, d.delay_mins AS delay_mins, d.recorded_at AS recorded_at \
         FROM delay_history d LEFT JOIN services s ON s.uid = d.uid",
    );
    spec.push_filters(&mut qb);
    qb.push(" ORDER BY d.recorded_at DESC LIMIT ");
    qb.push_bind(spec.limit);

    let rows = qb.build_query_as::<ObservationRow>().fetch_all(db).await?;
    let truncated = rows.len() as i64 >= spec.limit;

    let cells = rows
        .into_iter()
        .map(|r| {
            vec![
                r.uid.trim().to_string(),
                r.origin_crs.trim().to_string(),
                r.toc.unwrap_or_else(|| "—".into()),
                WEEKDAY_NAMES.get(r.weekday as usize).copied().unwrap_or("?").to_string(),
                format!("{:02}:00", r.departure_hour),
                r.delay_mins.to_string(),
                r.recorded_at.format("%Y-%m-%d %H:%M").to_string(),
            ]
        })
        .collect();

    Ok(ExploreResult {
        columns: ["Service", "Origin", "Operator", "Weekday", "Hour", "Delay (min)", "Recorded"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        rows: cells,
        truncated,
    })
}

fn subj_alias(subj: Subject) -> &'static str {
    match subj {
        Subject::Observations => "d",
        Subject::Predictions => "p",
        Subject::Cancellations => "c",
    }
}
