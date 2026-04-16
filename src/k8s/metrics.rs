//! On-demand controller metrics via native port-forward.
//!
//! Establishes a SPDY port-forward to a controller pod (no kubectl
//! required), fetches `/metrics`, and parses the Prometheus text format
//! into a `MetricsSnapshot` of the numbers we want to display.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use k8s_openapi::api::core::v1::Pod;
use kube::{Api, Client};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

/// Default Prometheus port for tofu-controller.
pub const DEFAULT_METRICS_PORT: u16 = 8080;

/// Snapshot of "headline" metrics for a single fetch. All fields are
/// `Option` so that the UI can show "-" when a metric isn't present.
#[derive(Default, Debug, Clone)]
pub struct MetricsSnapshot {
    pub fetched_at: Option<Instant>,
    pub fetch_ms: u128,

    // Controller-runtime gauges
    pub active_workers: Option<f64>,
    pub max_workers: Option<f64>,

    // Counter totals — per-minute rates derived in fill_rates()
    pub reconcile_success_total: Option<f64>,
    pub reconcile_error_total: Option<f64>,
    pub api_2xx_total: Option<f64>,
    pub api_non2xx_total: Option<f64>,

    // Histogram percentiles — at most one of value/off_scale_above is set per pair.
    pub p50_reconcile_secs: Option<f64>,
    pub p50_off_scale_above: Option<f64>,
    pub p95_reconcile_secs: Option<f64>,
    pub p95_off_scale_above: Option<f64>,
    pub p99_reconcile_secs: Option<f64>,
    pub p99_off_scale_above: Option<f64>,

    // Workqueue
    pub queue_depth_p0: Option<f64>,
    pub queue_depth_pneg100: Option<f64>,
    pub longest_running_secs: Option<f64>,

    // Per-resource
    pub tracked_resources: usize,

    // Computed per-minute rates (populated by fill_rates)
    pub reconcile_per_min: Option<f64>,
    pub error_per_min: Option<f64>,
    pub api_error_per_min: Option<f64>,
}

/// Counter values from the previous fetch, used to compute per-minute rates.
#[derive(Default, Debug, Clone)]
pub struct PrevCounters {
    pub at: Option<Instant>,
    pub reconcile_success: Option<f64>,
    pub reconcile_error: Option<f64>,
    pub api_non2xx: Option<f64>,
}

/// Fetch metrics from a controller pod and return a parsed snapshot.
///
/// The pod is selected by trying a list of common label selectors in order
/// (the helm chart uses `app.kubernetes.io/name=tofu-controller`; older
/// installs may use `app=tf-controller`). Set `pod_name` to skip selector
/// discovery and target a specific pod directly.
pub async fn fetch(
    client: &Client,
    namespace: &str,
    pod_name: Option<&str>,
    port: u16,
) -> Result<MetricsSnapshot> {
    let started = Instant::now();
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);

    let pod_name = match pod_name {
        Some(n) => n.to_string(),
        None => find_controller_pod(&pods).await?,
    };

    let mut pf = pods
        .portforward(&pod_name, &[port])
        .await
        .with_context(|| format!("port-forward to {namespace}/{pod_name}"))?;
    let mut stream = pf
        .take_stream(port)
        .ok_or_else(|| anyhow!("no stream for port {port}"))?;

    let request = b"GET /metrics HTTP/1.1\r\n\
                    Host: localhost\r\n\
                    User-Agent: terrarium\r\n\
                    Accept: text/plain\r\n\
                    Connection: close\r\n\
                    \r\n";
    stream
        .write_all(request)
        .await
        .context("writing HTTP request")?;
    stream.flush().await.ok();

    // Read until end of chunked HTTP body. The SPDY tunnel doesn't propagate
    // server-side close, so we look for the chunked terminator instead.
    let mut buf = Vec::with_capacity(1024 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    let mut headers_done = false;
    let mut chunked = false;
    let overall = Instant::now();
    loop {
        let read_fut = stream.read(&mut chunk);
        let n = match timeout(Duration::from_secs(2), read_fut).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(anyhow!("stream read error: {e}")),
            Err(_) => {
                if overall.elapsed() > Duration::from_secs(30) {
                    return Err(anyhow!("timed out reading /metrics response"));
                }
                continue;
            }
        };
        buf.extend_from_slice(&chunk[..n]);

        if !headers_done {
            if let Some(idx) = find_subseq(&buf, b"\r\n\r\n") {
                headers_done = true;
                let head = std::str::from_utf8(&buf[..idx]).unwrap_or("");
                chunked = head
                    .lines()
                    .any(|l| l.eq_ignore_ascii_case("transfer-encoding: chunked"));
            }
        }

        if headers_done && chunked && ends_with_final_chunk(&buf) {
            break;
        }
    }

    drop(stream);
    pf.abort();

    let split = find_subseq(&buf, b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let body_raw = &buf[split + 4..];
    let body_bytes = if chunked {
        decode_chunked(body_raw)?
    } else {
        body_raw.to_vec()
    };
    let body = std::str::from_utf8(&body_bytes)
        .map_err(|e| anyhow!("metrics body is not valid UTF-8: {e}"))?;

    let mut snap = parse_snapshot(body);
    snap.fetched_at = Some(started);
    snap.fetch_ms = started.elapsed().as_millis();
    Ok(snap)
}

/// Try a list of common label selectors and return the first matching
/// Running pod's name.
async fn find_controller_pod(pods: &Api<Pod>) -> Result<String> {
    const SELECTORS: &[&str] = &[
        "app.kubernetes.io/name=tofu-controller",
        "app.kubernetes.io/name=tf-controller",
        "app=tf-controller",
    ];

    for sel in SELECTORS {
        let lp = kube::api::ListParams::default().labels(sel);
        let list = pods.list(&lp).await?;
        let found = list.items.into_iter().find(|p| {
            p.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Running")
        });
        if let Some(pod) = found
            && let Some(name) = pod.metadata.name
        {
            return Ok(name);
        }
    }
    Err(anyhow!(
        "no Running controller pod found (tried selectors: {SELECTORS:?})"
    ))
}

/// Compute per-minute rates from the previous counter snapshot. Counter
/// resets (current < previous, e.g. controller restart) leave the rate as
/// `None` for that sample.
pub fn fill_rates(snap: &mut MetricsSnapshot, prev: &PrevCounters, now: Instant) {
    let dt = match prev.at {
        Some(t) => now.saturating_duration_since(t).as_secs_f64(),
        None => return,
    };
    if dt < 0.5 {
        return;
    }
    snap.reconcile_per_min =
        rate_per_min(prev.reconcile_success, snap.reconcile_success_total, dt);
    snap.error_per_min = rate_per_min(prev.reconcile_error, snap.reconcile_error_total, dt);
    snap.api_error_per_min = rate_per_min(prev.api_non2xx, snap.api_non2xx_total, dt);
}

/// Update `prev` with the current snapshot's counter totals.
pub fn update_prev(prev: &mut PrevCounters, snap: &MetricsSnapshot, now: Instant) {
    prev.at = Some(now);
    prev.reconcile_success = snap.reconcile_success_total;
    prev.reconcile_error = snap.reconcile_error_total;
    prev.api_non2xx = snap.api_non2xx_total;
}

fn rate_per_min(prev: Option<f64>, curr: Option<f64>, dt_secs: f64) -> Option<f64> {
    let (p, c) = (prev?, curr?);
    if c < p {
        return None; // counter reset
    }
    Some((c - p) / dt_secs * 60.0)
}

// ---------- Prometheus text parsing ----------

fn parse_snapshot(body: &str) -> MetricsSnapshot {
    let mut s = MetricsSnapshot::default();

    let find_one = |name: &str, must: &[(&str, &str)]| -> Option<f64> {
        for line in body.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if !starts_with_metric(line, name) {
                continue;
            }
            if must.iter().all(|(k, v)| has_label(line, k, v)) {
                return parse_value(line);
            }
        }
        None
    };

    let sum_where = |name: &str, must: &[(&str, &str)]| -> Option<f64> {
        let mut total = 0.0;
        let mut found = false;
        for line in body.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if !starts_with_metric(line, name) {
                continue;
            }
            if must.iter().all(|(k, v)| has_label(line, k, v)) {
                if let Some(v) = parse_value(line) {
                    total += v;
                    found = true;
                }
            }
        }
        if found { Some(total) } else { None }
    };

    let sum_where_pred = |name: &str,
                          must: &[(&str, &str)],
                          label: &str,
                          pred: &dyn Fn(&str) -> bool|
     -> Option<f64> {
        let mut total = 0.0;
        let mut found = false;
        for line in body.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if !starts_with_metric(line, name) {
                continue;
            }
            if !must.iter().all(|(k, v)| has_label(line, k, v)) {
                continue;
            }
            let lv = match label_value(line, label) {
                Some(x) => x,
                None => continue,
            };
            if pred(lv) {
                if let Some(v) = parse_value(line) {
                    total += v;
                    found = true;
                }
            }
        }
        if found { Some(total) } else { None }
    };

    s.active_workers = find_one(
        "controller_runtime_active_workers",
        &[("controller", "terraform")],
    );
    s.max_workers = find_one(
        "controller_runtime_max_concurrent_reconciles",
        &[("controller", "terraform")],
    );

    s.reconcile_error_total = sum_where(
        "controller_runtime_reconcile_total",
        &[("controller", "terraform"), ("result", "error")],
    );
    s.reconcile_success_total = sum_where_pred(
        "controller_runtime_reconcile_total",
        &[("controller", "terraform")],
        "result",
        &|v| v != "error",
    );

    s.api_2xx_total =
        sum_where_pred("rest_client_requests_total", &[], "code", &|v| {
            v.starts_with('2')
        });
    s.api_non2xx_total =
        sum_where_pred("rest_client_requests_total", &[], "code", &|v| {
            !v.starts_with('2')
        });

    let quantile = |q: f64| {
        histogram_quantile_aggregated(
            body,
            "gotk_reconcile_duration_seconds_bucket",
            &[("kind", "Terraform")],
            q,
        )
    };
    match quantile(0.50) {
        QuantileResult::Value(v) => s.p50_reconcile_secs = Some(v),
        QuantileResult::OffScale(le) => s.p50_off_scale_above = Some(le),
        QuantileResult::None => {}
    }
    match quantile(0.95) {
        QuantileResult::Value(v) => s.p95_reconcile_secs = Some(v),
        QuantileResult::OffScale(le) => s.p95_off_scale_above = Some(le),
        QuantileResult::None => {}
    }
    match quantile(0.99) {
        QuantileResult::Value(v) => s.p99_reconcile_secs = Some(v),
        QuantileResult::OffScale(le) => s.p99_off_scale_above = Some(le),
        QuantileResult::None => {}
    }

    s.queue_depth_p0 = sum_where(
        "workqueue_depth",
        &[("controller", "terraform"), ("priority", "0")],
    );
    s.queue_depth_pneg100 = sum_where(
        "workqueue_depth",
        &[("controller", "terraform"), ("priority", "-100")],
    );
    s.longest_running_secs = find_one(
        "workqueue_longest_running_processor_seconds",
        &[("controller", "terraform")],
    );

    let mut names = HashSet::new();
    for line in body.lines() {
        if !line.starts_with("gotk_reconcile_duration_seconds_count{") {
            continue;
        }
        if !has_label(line, "kind", "Terraform") {
            continue;
        }
        if let (Some(ns), Some(n)) = (label_value(line, "namespace"), label_value(line, "name")) {
            names.insert(format!("{ns}/{n}"));
        }
    }
    s.tracked_resources = names.len();

    s
}

#[derive(Debug, Clone, Copy)]
enum QuantileResult {
    Value(f64),
    OffScale(f64),
    None,
}

fn histogram_quantile_aggregated(
    body: &str,
    bucket_metric: &str,
    must: &[(&str, &str)],
    q: f64,
) -> QuantileResult {
    let mut agg: BTreeMap<OrderedFloat, f64> = BTreeMap::new();
    for line in body.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !starts_with_metric(line, bucket_metric) {
            continue;
        }
        if !must.iter().all(|(k, v)| has_label(line, k, v)) {
            continue;
        }
        let le_str = match label_value(line, "le") {
            Some(x) => x,
            None => continue,
        };
        let le: f64 = if le_str == "+Inf" {
            f64::INFINITY
        } else {
            match le_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            }
        };
        let count = match parse_value(line) {
            Some(v) => v,
            None => continue,
        };
        *agg.entry(OrderedFloat(le)).or_insert(0.0) += count;
    }
    if agg.is_empty() {
        return QuantileResult::None;
    }
    let buckets: Vec<(f64, f64)> = agg.into_iter().map(|(k, v)| (k.0, v)).collect();
    let total = match buckets.last() {
        Some((_, c)) => *c,
        None => return QuantileResult::None,
    };
    if total <= 0.0 {
        return QuantileResult::None;
    }
    let target = q * total;

    let mut prev_le = 0.0;
    let mut prev_count = 0.0;
    let largest_finite_le = buckets
        .iter()
        .filter(|(le, _)| le.is_finite())
        .map(|(le, _)| *le)
        .last()
        .unwrap_or(0.0);
    for (le, count) in &buckets {
        if *count >= target {
            if le.is_infinite() {
                return QuantileResult::OffScale(largest_finite_le);
            }
            let bucket_size = count - prev_count;
            if bucket_size <= 0.0 {
                return QuantileResult::Value(*le);
            }
            let position = target - prev_count;
            return QuantileResult::Value(
                prev_le + (position / bucket_size) * (le - prev_le),
            );
        }
        prev_le = *le;
        prev_count = *count;
    }
    QuantileResult::None
}

#[derive(Copy, Clone, PartialEq, PartialOrd)]
struct OrderedFloat(f64);
impl Eq for OrderedFloat {}
impl Ord for OrderedFloat {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0
            .partial_cmp(&other.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

fn parse_value(line: &str) -> Option<f64> {
    line.rsplit_once(' ').and_then(|(_, v)| v.parse().ok())
}

fn label_value<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    let needle = format!("{label}=\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn starts_with_metric(line: &str, name: &str) -> bool {
    if !line.starts_with(name) {
        return false;
    }
    let next = line.as_bytes().get(name.len()).copied();
    matches!(next, Some(b'{') | Some(b' '))
}

fn has_label(line: &str, key: &str, value: &str) -> bool {
    let needle = format!("{key}=\"{value}\"");
    line.contains(&needle)
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn ends_with_final_chunk(buf: &[u8]) -> bool {
    buf.ends_with(b"\r\n0\r\n\r\n") || buf.ends_with(b"0\r\n\r\n")
}

fn decode_chunked(body: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0usize;
    while i < body.len() {
        let line_end = find_subseq(&body[i..], b"\r\n")
            .ok_or_else(|| anyhow!("chunked: no CRLF after size"))?;
        let size_line = std::str::from_utf8(&body[i..i + line_end])
            .map_err(|_| anyhow!("chunked: non-utf8 size line"))?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|e| anyhow!("chunked: bad size '{size_hex}': {e}"))?;
        i += line_end + 2;
        if size == 0 {
            return Ok(out);
        }
        if i + size > body.len() {
            return Err(anyhow!("chunked: truncated chunk"));
        }
        out.extend_from_slice(&body[i..i + size]);
        i += size;
        if body.get(i..i + 2) != Some(b"\r\n") {
            return Err(anyhow!("chunked: missing CRLF after chunk"));
        }
        i += 2;
    }
    Err(anyhow!("chunked: ended without zero-length chunk"))
}
