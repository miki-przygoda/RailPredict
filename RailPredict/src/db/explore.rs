//! Query Explorer — a guided, constrained "fill-in-the-gaps" data explorer.
//!
//! The admin composes a query from whitelisted dropdowns/chips (subject, filters,
//! group-by, metric) and the server runs a **safe parameterized** query. There is
//! NO raw SQL: every SQL fragment (the metric expression, the group-by column, the
//! join) is selected by a `match` on an enum — never interpolated from user text —
//! and every user value is a bound parameter (`push_bind`) via `sqlx::QueryBuilder`
//! (the same pattern as `db/history.rs`).
//!
//! v1 subject is `observations` (`delay_history ⋈ services`), which covers the
//! canonical example "trains between 06:00–10:00 where operator = Heathrow Express,
//! grouped by hour". A `predictions` subject can be added later behind the same enum.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, QueryBuilder};

use super::Db;

// ---------------------------------------------------------------------------
// Whitelisted spec
// ---------------------------------------------------------------------------

/// What to group results by (the `GROUP BY` dimension). `None` = a flat list /
/// single overall aggregate.
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
    /// The SELECT expression for the group label (fixed, never user text).
    fn label_expr(self) -> &'static str {
        match self {
            Self::None => "'All'",
            Self::Operator => "COALESCE(s.toc, 'Unknown')",
            Self::Origin => "d.origin_crs",
            Self::Destination => "COALESCE(s.destination_crs, '?')",
            Self::Route => "d.origin_crs || ' \u{2192} ' || COALESCE(s.destination_crs, '?')",
            Self::Hour => "LPAD(d.departure_hour::text, 2, '0') || ':00'",
            Self::Weekday => {
                "(ARRAY['Mon','Tue','Wed','Thu','Fri','Sat','Sun'])[d.weekday + 1]"
            }
            Self::Day => "to_char(d.recorded_at AT TIME ZONE 'Europe/London', 'YYYY-MM-DD')",
        }
    }
    /// The GROUP BY clause (fixed), or `None` for a single overall aggregate.
    fn group_clause(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Operator => Some("s.toc"),
            Self::Origin => Some("d.origin_crs"),
            Self::Destination => Some("s.destination_crs"),
            Self::Route => Some("d.origin_crs, s.destination_crs"),
            Self::Hour => Some("d.departure_hour"),
            Self::Weekday => Some("d.weekday"),
            Self::Day => Some("(d.recorded_at AT TIME ZONE 'Europe/London')::date"),
        }
    }
    /// Inverse of [`GroupBy::parse`] — the query-string key for this variant.
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
    fn label_header(self) -> &'static str {
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
    /// Whether this grouping needs the `services` join.
    fn needs_services(self) -> bool {
        matches!(self, Self::Operator | Self::Destination | Self::Route)
    }
}

/// What to measure. `List` returns matching observation rows; the rest aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    List,
    Count,
    AvgDelay,
    OnTimePct,
}

impl Metric {
    pub fn parse(s: &str) -> Self {
        match s {
            "count" => Self::Count,
            "avg_delay" => Self::AvgDelay,
            "on_time_pct" => Self::OnTimePct,
            _ => Self::List,
        }
    }
    /// The aggregate SELECT expression (fixed). Only meaningful when not `List`.
    fn value_expr(self) -> &'static str {
        match self {
            Self::Count => "COUNT(*)::float8",
            Self::AvgDelay => "AVG(d.delay_mins)::float8",
            Self::OnTimePct => "(AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8",
            Self::List => "0::float8",
        }
    }
    /// Inverse of [`Metric::parse`] — the query-string key for this variant.
    pub fn key(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Count => "count",
            Self::AvgDelay => "avg_delay",
            Self::OnTimePct => "on_time_pct",
        }
    }
    fn header(self) -> &'static str {
        match self {
            Self::Count => "Count",
            Self::AvgDelay => "Avg delay (min)",
            Self::OnTimePct => "On-time %",
            Self::List => "",
        }
    }
}

/// Maximum rows any explore query may return.
const MAX_LIMIT: i64 = 500;
const DEFAULT_LIMIT: i64 = 200;
const DEFAULT_WINDOW_HOURS: i32 = 168; // 7 days

/// A validated explore query. Every field is already range-checked / whitelisted;
/// construct only via [`ExploreSpec::from_raw`].
#[derive(Debug, Clone)]
pub struct ExploreSpec {
    pub window_hours: i32,
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
    /// Validate raw (untrusted) query-string values into a safe spec. Out-of-range
    /// or unparseable values are clamped or dropped — never an error, never raw SQL.
    #[allow(clippy::too_many_arguments)]
    pub fn from_raw(
        window: Option<&str>,
        from_hour: Option<i16>,
        to_hour: Option<i16>,
        weekdays: Option<&str>,
        operator: Option<&str>,
        origin: Option<&str>,
        destination: Option<&str>,
        min_delay: Option<i32>,
        group: Option<&str>,
        metric: Option<&str>,
        limit: Option<i64>,
    ) -> Self {
        let window_hours = match window {
            Some("24h") => 24,
            Some("7d") => 168,
            Some("30d") => 720,
            Some("all") => 24 * 365 * 100,
            _ => DEFAULT_WINDOW_HOURS,
        };
        let clamp_hour = |h: i16| h.clamp(0, 23);
        let weekdays: Vec<i16> = weekdays
            .unwrap_or("")
            .split(',')
            .filter_map(|t| t.trim().parse::<i16>().ok())
            .filter(|d| (0..=6).contains(d))
            .collect();
        // CRS codes are exactly 3 letters; ignore anything else (the filter is simply
        // skipped). TOC is bound as-is (still injection-safe via push_bind).
        let crs = |c: Option<&str>| -> Option<String> {
            c.map(|s| s.trim().to_uppercase())
                .filter(|s| s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic()))
        };
        Self {
            window_hours,
            from_hour: from_hour.map(clamp_hour),
            to_hour: to_hour.map(clamp_hour),
            weekdays,
            operator: operator.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            origin: crs(origin),
            destination: crs(destination),
            min_delay,
            group_by: group.map(GroupBy::parse).unwrap_or_default(),
            metric: metric.map(Metric::parse).unwrap_or_default(),
            limit: limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        }
    }

    fn needs_services(&self) -> bool {
        self.group_by.needs_services()
            || self.operator.is_some()
            || self.destination.is_some()
    }

    /// Push the shared WHERE clauses (window, sanity filter, and every active
    /// filter) as bound parameters.
    fn push_filters(&self, qb: &mut QueryBuilder<'_, sqlx::Postgres>) {
        qb.push(" WHERE d.recorded_at > NOW() - ");
        qb.push_bind(self.window_hours);
        qb.push("::INT * INTERVAL '1 hour' AND d.delay_mins BETWEEN -120 AND 600");
        if let Some(h) = self.from_hour {
            qb.push(" AND d.departure_hour >= ");
            qb.push_bind(h);
        }
        if let Some(h) = self.to_hour {
            qb.push(" AND d.departure_hour <= ");
            qb.push_bind(h);
        }
        if !self.weekdays.is_empty() {
            qb.push(" AND d.weekday = ANY(");
            qb.push_bind(self.weekdays.clone());
            qb.push(")");
        }
        if let Some(op) = &self.operator {
            qb.push(" AND s.toc = ");
            qb.push_bind(op.clone());
        }
        if let Some(o) = &self.origin {
            qb.push(" AND d.origin_crs = ");
            qb.push_bind(o.clone());
        }
        if let Some(dst) = &self.destination {
            qb.push(" AND s.destination_crs = ");
            qb.push_bind(dst.clone());
        }
        if let Some(md) = self.min_delay {
            qb.push(" AND d.delay_mins >= ");
            qb.push_bind(md);
        }
    }
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// A generic tabular result the frontend renders as a table (charts can layer on
/// top later). `truncated` is set when the row cap was hit.
#[derive(Debug, Clone)]
pub struct ExploreResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
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
    if spec.metric == Metric::List {
        run_list(db, spec).await
    } else {
        run_grouped(db, spec).await
    }
}

async fn run_grouped(db: &Db, spec: &ExploreSpec) -> sqlx::Result<ExploreResult> {
    let mut qb = QueryBuilder::<sqlx::Postgres>::new("SELECT ");
    qb.push(spec.group_by.label_expr());
    qb.push(" AS label, ");
    qb.push(spec.metric.value_expr());
    qb.push(" AS value, COUNT(*) AS n FROM delay_history d");
    if spec.needs_services() {
        qb.push(" LEFT JOIN services s ON s.uid = d.uid");
    }
    spec.push_filters(&mut qb);
    if let Some(g) = spec.group_by.group_clause() {
        qb.push(" GROUP BY ");
        qb.push(g);
    }
    qb.push(" ORDER BY n DESC LIMIT ");
    qb.push_bind(spec.limit);

    let rows = qb.build_query_as::<GroupedRow>().fetch_all(db).await?;
    let truncated = rows.len() as i64 >= spec.limit;

    let value_fmt = |v: Option<f64>| match (spec.metric, v) {
        (Metric::Count, Some(x)) => format!("{}", x as i64),
        (_, Some(x)) => format!("{x:.1}"),
        (_, None) => "—".to_string(),
    };
    let cells = rows
        .into_iter()
        .map(|r| {
            vec![
                r.label.unwrap_or_else(|| "—".into()),
                value_fmt(r.value),
                r.n.to_string(),
            ]
        })
        .collect();

    Ok(ExploreResult {
        columns: vec![
            spec.group_by.label_header().to_string(),
            spec.metric.header().to_string(),
            "Observations".to_string(),
        ],
        rows: cells,
        truncated,
    })
}

async fn run_list(db: &Db, spec: &ExploreSpec) -> sqlx::Result<ExploreResult> {
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
