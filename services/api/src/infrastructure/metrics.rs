//! Process-local metrics with Prometheus text exposition.
//!
//! Hand-rolled on purpose: the metric set is small and fixed, and avoiding a
//! metrics crate keeps the dependency tree auditable. HTTP labels come from
//! the matched route template (`MatchedPath`), never the raw path, so
//! cardinality stays bounded.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Cumulative HTTP duration in microseconds plus sample count, per
/// (method, route).
#[derive(Debug, Default, Clone, Copy)]
struct DurationSum {
    micros: u64,
    count: u64,
}

#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    /// (method, route, status class) -> count.
    http_requests: Mutex<HashMap<(String, String, String), u64>>,
    /// (method, route) -> cumulative duration.
    http_duration: Mutex<HashMap<(String, String), DurationSum>>,
    /// Current live WebSocket connections.
    pub ws_connections: AtomicI64,
    /// WebSocket connections accepted since start.
    pub ws_connections_total: AtomicU64,
    /// Moves committed to the database (idempotent replays not counted).
    pub moves_committed: AtomicU64,
    /// Games created through the lobby.
    pub games_created: AtomicU64,
    /// Outbox events published after commit.
    pub outbox_published: AtomicU64,
    /// Games paired by the matchmaking scheduler.
    pub matchmaking_pairs: AtomicU64,
    pub auth_logins: AtomicU64,
    pub auth_registrations: AtomicU64,
    pub auth_login_failures: AtomicU64,
    /// Requests/commands rejected by a rate limiter.
    pub rate_limited: AtomicU64,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            http_requests: Mutex::new(HashMap::new()),
            http_duration: Mutex::new(HashMap::new()),
            ws_connections: AtomicI64::new(0),
            ws_connections_total: AtomicU64::new(0),
            moves_committed: AtomicU64::new(0),
            games_created: AtomicU64::new(0),
            outbox_published: AtomicU64::new(0),
            matchmaking_pairs: AtomicU64::new(0),
            auth_logins: AtomicU64::new(0),
            auth_registrations: AtomicU64::new(0),
            auth_login_failures: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
        }
    }

    /// Record one served HTTP request. `route` must be a *template*
    /// (e.g. `/api/games/{id}`), not an expanded path.
    pub fn record_http(&self, method: &str, route: &str, status: u16, elapsed_micros: u64) {
        let class = match status {
            200..=299 => "2xx",
            300..=399 => "3xx",
            400..=499 => "4xx",
            _ => "5xx",
        };
        *self
            .http_requests
            .lock()
            .expect("metrics lock poisoned")
            .entry((method.to_string(), route.to_string(), class.to_string()))
            .or_insert(0) += 1;

        let mut durations = self.http_duration.lock().expect("metrics lock poisoned");
        let entry = durations
            .entry((method.to_string(), route.to_string()))
            .or_default();
        entry.micros = entry.micros.saturating_add(elapsed_micros);
        entry.count += 1;
    }

    /// Render every metric in the Prometheus text exposition format.
    pub fn render(&self) -> String {
        let mut out = String::with_capacity(4_096);
        macro_rules! scalar {
            ($help:expr, $ty:expr, $name:expr, $value:expr) => {{
                out.push_str(concat!("# HELP ", $name, " ", $help, "\n"));
                out.push_str(concat!("# TYPE ", $name, " ", $ty, "\n"));
                out.push_str(&$name);
                out.push(' ');
                out.push_str(&$value.to_string());
                out.push('\n');
            }};
        }

        scalar!(
            "1 if the process is serving, always 1.",
            "gauge",
            "nenchess_up",
            1
        );
        scalar!(
            "Seconds since the process started serving.",
            "gauge",
            "nenchess_uptime_seconds",
            self.started.elapsed().as_secs()
        );
        out.push_str("# HELP nenchess_build_info Build version of the API binary.\n");
        out.push_str("# TYPE nenchess_build_info gauge\n");
        out.push_str(&format!(
            "nenchess_build_info{{version=\"{}\"}} 1\n",
            env!("CARGO_PKG_VERSION")
        ));

        // HTTP counters, sorted for deterministic output.
        let requests = self.http_requests.lock().expect("metrics lock poisoned");
        let mut keys: Vec<_> = requests.keys().cloned().collect();
        keys.sort();
        if !keys.is_empty() {
            out.push_str(
                "# HELP nenchess_http_requests_total \
                 HTTP requests by method, matched route and status class.\n",
            );
            out.push_str("# TYPE nenchess_http_requests_total counter\n");
        }
        for (method, route, class) in &keys {
            let count = requests[&(method.clone(), route.clone(), class.clone())];
            out.push_str(&format!(
                "nenchess_http_requests_total{{method=\"{}\",route=\"{}\",status=\"{}\"}} {}\n",
                escape(method),
                escape(route),
                escape(class),
                count
            ));
        }
        drop(requests);

        let durations = self.http_duration.lock().expect("metrics lock poisoned");
        let mut keys: Vec<_> = durations.keys().cloned().collect();
        keys.sort();
        if !keys.is_empty() {
            out.push_str(
                "# HELP nenchess_http_request_duration_seconds_sum \
                 Cumulative served-request duration.\n",
            );
            out.push_str("# TYPE nenchess_http_request_duration_seconds_sum gauge\n");
            for (method, route) in &keys {
                let sum = durations[&(method.clone(), route.clone())];
                out.push_str(&format!(
                    "nenchess_http_request_duration_seconds_sum{{method=\"{}\",route=\"{}\"}} {:.6}\n",
                    escape(method),
                    escape(route),
                    sum.micros as f64 / 1_000_000.0
                ));
            }
            out.push_str(
                "# HELP nenchess_http_request_duration_seconds_count \
                 Served requests included in the duration sum.\n",
            );
            out.push_str("# TYPE nenchess_http_request_duration_seconds_count gauge\n");
            for (method, route) in &keys {
                let sum = durations[&(method.clone(), route.clone())];
                out.push_str(&format!(
                    "nenchess_http_request_duration_seconds_count{{method=\"{}\",route=\"{}\"}} {}\n",
                    escape(method),
                    escape(route),
                    sum.count
                ));
            }
        }
        drop(durations);

        scalar!(
            "Current live WebSocket connections.",
            "gauge",
            "nenchess_ws_connections",
            self.ws_connections.load(Ordering::Relaxed)
        );
        scalar!(
            "WebSocket connections accepted since start.",
            "counter",
            "nenchess_ws_connections_total",
            self.ws_connections_total.load(Ordering::Relaxed)
        );
        scalar!(
            "Moves durably committed to the database.",
            "counter",
            "nenchess_moves_committed",
            self.moves_committed.load(Ordering::Relaxed)
        );
        scalar!(
            "Games created through the lobby.",
            "counter",
            "nenchess_games_created",
            self.games_created.load(Ordering::Relaxed)
        );
        scalar!(
            "Outbox events published after commit.",
            "counter",
            "nenchess_outbox_published",
            self.outbox_published.load(Ordering::Relaxed)
        );
        scalar!(
            "Games paired by the matchmaking scheduler.",
            "counter",
            "nenchess_matchmaking_pairs",
            self.matchmaking_pairs.load(Ordering::Relaxed)
        );
        scalar!(
            "Successful logins.",
            "counter",
            "nenchess_auth_logins_total",
            self.auth_logins.load(Ordering::Relaxed)
        );
        scalar!(
            "Successful registrations.",
            "counter",
            "nenchess_auth_registrations_total",
            self.auth_registrations.load(Ordering::Relaxed)
        );
        scalar!(
            "Rejected login attempts.",
            "counter",
            "nenchess_auth_login_failures_total",
            self.auth_login_failures.load(Ordering::Relaxed)
        );
        scalar!(
            "Requests or commands rejected by a rate limiter.",
            "counter",
            "nenchess_rate_limited_total",
            self.rate_limited.load(Ordering::Relaxed)
        );
        out
    }
}

/// Escape a label value per the Prometheus exposition format.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_is_prometheus_text_and_deterministic() {
        let metrics = Metrics::new();
        metrics.record_http("GET", "/api/games", 200, 1_500);
        metrics.record_http("GET", "/api/games", 200, 500);
        metrics.record_http("POST", "/api/games", 401, 300);
        metrics.moves_committed.fetch_add(7, Ordering::Relaxed);

        let first = metrics.render();
        assert_eq!(first, metrics.render());
        assert!(first.contains(
            "nenchess_http_requests_total{method=\"GET\",route=\"/api/games\",status=\"2xx\"} 2"
        ));
        assert!(first.contains(
            "nenchess_http_requests_total{method=\"POST\",route=\"/api/games\",status=\"4xx\"} 1"
        ));
        assert!(first.contains(
            "nenchess_http_request_duration_seconds_sum{method=\"GET\",route=\"/api/games\"} 0.002000"
        ));
        assert!(first.contains("nenchess_moves_committed 7"));
        assert!(first.contains("nenchess_up 1"));
        assert!(first.contains("nenchess_build_info{version=\""));
        // Every TYPE line is preceded by its HELP line.
        for line in first.lines() {
            if line.starts_with("# TYPE ") {
                assert!(line.split_whitespace().count() == 4);
            }
        }
    }

    #[test]
    fn label_values_are_escaped() {
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
