use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RECENT_SPAN_LIMIT: usize = 512;
const RATE_WINDOW_SECONDS: u64 = 10;
const STARTUP_REPORT_PATH_ENV: &str = "SPACESLY_PERFORMANCE_STARTUP_REPORT";
const LATENCY_BUCKET_US: [u64; 23] = [
    50,
    100,
    250,
    500,
    1_000,
    2_000,
    4_000,
    8_000,
    16_000,
    32_000,
    64_000,
    125_000,
    250_000,
    500_000,
    1_000_000,
    2_000_000,
    4_000_000,
    8_000_000,
    15_000_000,
    30_000_000,
    60_000_000,
    120_000_000,
    300_000_000,
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceMode {
    #[default]
    Normal,
    Profiling,
}

impl PerformanceMode {
    fn as_u8(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Profiling => 1,
        }
    }

    fn from_u8(value: u8) -> Self {
        if value == 1 {
            Self::Profiling
        } else {
            Self::Normal
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FrontendStartupMetrics {
    pub frontend_boot_ms: f64,
    pub hydration_ms: f64,
    pub workspace_boot_ms: f64,
    pub first_interactive_frame_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StartupMetrics {
    pub backend_start_ms: Option<f64>,
    pub frontend_boot_ms: Option<f64>,
    pub hydration_ms: Option<f64>,
    pub workspace_boot_ms: Option<f64>,
    pub first_interactive_frame_ms: Option<f64>,
    pub tti_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MetricSummary {
    pub name: String,
    pub category: String,
    pub unit: String,
    pub count: u64,
    pub average: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct CounterSummary {
    pub name: String,
    pub category: String,
    pub total: u64,
    pub per_second: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecentSpan {
    pub name: String,
    pub category: String,
    pub duration_ms: f64,
    pub recorded_at_ms: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct InstrumentationOverhead {
    pub samples: u64,
    pub average_ns: u64,
    pub max_ns: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProcessResources {
    pub rss_bytes: Option<u64>,
    pub cpu_percent: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PerformanceSnapshot {
    pub schema_version: u32,
    pub app_version: String,
    pub generated_at_ms: u64,
    pub uptime_ms: u64,
    pub mode: PerformanceMode,
    pub environment: BTreeMap<String, String>,
    pub startup: StartupMetrics,
    pub resources: ProcessResources,
    pub metrics: Vec<MetricSummary>,
    pub counters: Vec<CounterSummary>,
    pub recent_spans: Vec<RecentSpan>,
    pub instrumentation_overhead: InstrumentationOverhead,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MetricKey {
    category: &'static str,
    name: &'static str,
}

#[derive(Clone, Debug, Default)]
struct Histogram {
    count: u64,
    total_us: u128,
    max_us: u64,
    buckets: [u64; LATENCY_BUCKET_US.len()],
}

impl Histogram {
    fn record(&mut self, duration: Duration) {
        let micros = duration.as_micros().min(u64::MAX as u128) as u64;
        self.count = self.count.saturating_add(1);
        self.total_us = self.total_us.saturating_add(micros as u128);
        self.max_us = self.max_us.max(micros);
        let bucket = LATENCY_BUCKET_US
            .iter()
            .position(|upper| micros <= *upper)
            .unwrap_or(LATENCY_BUCKET_US.len() - 1);
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
    }

    fn percentile_ms(&self, percentile: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let target = ((self.count as f64 * percentile).ceil() as u64).max(1);
        let mut observed = 0_u64;
        for (index, count) in self.buckets.iter().enumerate() {
            observed = observed.saturating_add(*count);
            if observed >= target {
                return LATENCY_BUCKET_US[index] as f64 / 1_000.0;
            }
        }
        self.max_us as f64 / 1_000.0
    }
}

#[derive(Clone, Debug, Default)]
struct RateCounter {
    total: u64,
    seconds: VecDeque<(u64, u64)>,
}

impl RateCounter {
    fn increment(&mut self, now_second: u64, amount: u64) {
        self.total = self.total.saturating_add(amount);
        if let Some((second, count)) = self.seconds.back_mut() {
            if *second == now_second {
                *count = count.saturating_add(amount);
            } else {
                self.seconds.push_back((now_second, amount));
            }
        } else {
            self.seconds.push_back((now_second, amount));
        }
        while self
            .seconds
            .front()
            .is_some_and(|(second, _)| now_second.saturating_sub(*second) >= RATE_WINDOW_SECONDS)
        {
            self.seconds.pop_front();
        }
    }

    fn rate(&self, now_second: u64) -> f64 {
        let count = self
            .seconds
            .iter()
            .filter(|(second, _)| now_second.saturating_sub(*second) < RATE_WINDOW_SECONDS)
            .map(|(_, count)| *count)
            .sum::<u64>();
        count as f64 / RATE_WINDOW_SECONDS as f64
    }
}

#[derive(Default)]
struct RegistryState {
    histograms: BTreeMap<MetricKey, Histogram>,
    counters: BTreeMap<MetricKey, RateCounter>,
    recent_spans: VecDeque<RecentSpan>,
    startup: StartupMetrics,
}

pub struct PerformanceRegistry {
    started_at: Instant,
    mode: AtomicU8,
    state: Mutex<RegistryState>,
    overhead_total_ns: AtomicU64,
    overhead_max_ns: AtomicU64,
    overhead_samples: AtomicU64,
    cpu_sample: Mutex<Option<(Instant, u64)>>,
}

impl Default for PerformanceRegistry {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            mode: AtomicU8::new(PerformanceMode::Normal.as_u8()),
            state: Mutex::new(RegistryState::default()),
            overhead_total_ns: AtomicU64::new(0),
            overhead_max_ns: AtomicU64::new(0),
            overhead_samples: AtomicU64::new(0),
            cpu_sample: Mutex::new(None),
        }
    }
}

static PERFORMANCE: OnceLock<PerformanceRegistry> = OnceLock::new();

pub fn registry() -> &'static PerformanceRegistry {
    PERFORMANCE.get_or_init(PerformanceRegistry::default)
}

pub fn initialize() {
    let _ = registry();
}

pub fn mark_backend_ready() {
    let registry = registry();
    if let Ok(mut state) = registry.state.lock() {
        state.startup.backend_start_ms =
            Some(registry.started_at.elapsed().as_secs_f64() * 1_000.0);
    }
}

pub fn record_frontend_startup(metrics: FrontendStartupMetrics) {
    let registry = registry();
    if let Ok(mut state) = registry.state.lock() {
        if state.startup.tti_ms.is_some() {
            return;
        }
        state.startup.frontend_boot_ms = Some(metrics.frontend_boot_ms);
        state.startup.hydration_ms = Some(metrics.hydration_ms);
        state.startup.workspace_boot_ms = Some(metrics.workspace_boot_ms);
        state.startup.first_interactive_frame_ms = Some(metrics.first_interactive_frame_ms);
        state.startup.tti_ms = Some(registry.started_at.elapsed().as_secs_f64() * 1_000.0);
    }
}

pub fn set_mode(mode: PerformanceMode) {
    let registry = registry();
    registry.mode.store(mode.as_u8(), Ordering::Relaxed);
    if mode == PerformanceMode::Normal {
        if let Ok(mut state) = registry.state.lock() {
            state.recent_spans.clear();
        }
    }
}

pub fn mode() -> PerformanceMode {
    PerformanceMode::from_u8(registry().mode.load(Ordering::Relaxed))
}

pub fn increment(name: &'static str, category: &'static str, amount: u64) {
    let registry = registry();
    let overhead_started = Instant::now();
    if let Ok(mut state) = registry.state.lock() {
        let now_second = registry.started_at.elapsed().as_secs();
        state
            .counters
            .entry(MetricKey { category, name })
            .or_default()
            .increment(now_second, amount);
    }
    registry.record_overhead(overhead_started.elapsed());
}

pub fn record_duration(name: &'static str, category: &'static str, duration: Duration) {
    record_duration_with_context(name, category, duration, BTreeMap::new());
}

pub fn record_sqlite_lock_wait(duration: Duration) {
    record_duration("sqlite_lock_wait_ms", "sqlite", duration);
    if duration >= Duration::from_millis(1) {
        increment("sqlite_busy_count", "sqlite", 1);
    }
}

pub fn record_duration_with_context(
    name: &'static str,
    category: &'static str,
    duration: Duration,
    context: BTreeMap<String, String>,
) {
    let registry = registry();
    let overhead_started = Instant::now();
    if let Ok(mut state) = registry.state.lock() {
        state
            .histograms
            .entry(MetricKey { category, name })
            .or_default()
            .record(duration);
        if category.starts_with("sqlite_read") {
            state
                .histograms
                .entry(MetricKey {
                    category: "sqlite",
                    name: "sqlite_read_latency_ms",
                })
                .or_default()
                .record(duration);
        } else if category.starts_with("sqlite_write") {
            state
                .histograms
                .entry(MetricKey {
                    category: "sqlite",
                    name: "sqlite_write_latency_ms",
                })
                .or_default()
                .record(duration);
        }
        if category.ends_with("transaction") {
            state
                .histograms
                .entry(MetricKey {
                    category: "sqlite",
                    name: "sqlite_transaction_duration_ms",
                })
                .or_default()
                .record(duration);
        }
        if registry.mode.load(Ordering::Relaxed) == PerformanceMode::Profiling.as_u8() {
            if state.recent_spans.len() == RECENT_SPAN_LIMIT {
                state.recent_spans.pop_front();
            }
            state.recent_spans.push_back(RecentSpan {
                name: name.to_string(),
                category: category.to_string(),
                duration_ms: duration.as_secs_f64() * 1_000.0,
                recorded_at_ms: registry.started_at.elapsed().as_millis() as u64,
                context,
            });
        }
    }
    registry.record_overhead(overhead_started.elapsed());
}

pub struct PerformanceSpan {
    name: &'static str,
    category: &'static str,
    started_at: Instant,
    context: BTreeMap<String, String>,
    finished: bool,
}

impl PerformanceSpan {
    pub fn with_context(mut self, key: &str, value: impl Into<String>) -> Self {
        if mode() == PerformanceMode::Profiling {
            self.context.insert(key.to_string(), value.into());
        }
        self
    }

    pub fn finish(mut self) -> Duration {
        let elapsed = self.started_at.elapsed();
        record_duration_with_context(
            self.name,
            self.category,
            elapsed,
            std::mem::take(&mut self.context),
        );
        self.finished = true;
        elapsed
    }
}

impl Drop for PerformanceSpan {
    fn drop(&mut self) {
        if !self.finished {
            record_duration_with_context(
                self.name,
                self.category,
                self.started_at.elapsed(),
                std::mem::take(&mut self.context),
            );
        }
    }
}

pub fn span(name: &'static str, category: &'static str) -> PerformanceSpan {
    PerformanceSpan {
        name,
        category,
        started_at: Instant::now(),
        context: BTreeMap::new(),
        finished: false,
    }
}

pub fn reset() {
    let registry = registry();
    if let Ok(mut state) = registry.state.lock() {
        let startup = state.startup.clone();
        *state = RegistryState {
            startup,
            ..RegistryState::default()
        };
    }
    registry.overhead_total_ns.store(0, Ordering::Relaxed);
    registry.overhead_max_ns.store(0, Ordering::Relaxed);
    registry.overhead_samples.store(0, Ordering::Relaxed);
    if let Ok(mut sample) = registry.cpu_sample.lock() {
        *sample = None;
    }
}

pub fn snapshot() -> PerformanceSnapshot {
    let registry = registry();
    let now_second = registry.started_at.elapsed().as_secs();
    let (startup, metrics, counters, recent_spans) = registry
        .state
        .lock()
        .map(|state| {
            let metrics = state
                .histograms
                .iter()
                .map(|(key, histogram)| MetricSummary {
                    name: key.name.to_string(),
                    category: key.category.to_string(),
                    unit: "ms".to_string(),
                    count: histogram.count,
                    average: if histogram.count == 0 {
                        0.0
                    } else {
                        histogram.total_us as f64 / histogram.count as f64 / 1_000.0
                    },
                    p50: histogram.percentile_ms(0.50),
                    p95: histogram.percentile_ms(0.95),
                    p99: histogram.percentile_ms(0.99),
                    max: histogram.max_us as f64 / 1_000.0,
                })
                .collect();
            let counters = state
                .counters
                .iter()
                .map(|(key, counter)| CounterSummary {
                    name: key.name.to_string(),
                    category: key.category.to_string(),
                    total: counter.total,
                    per_second: counter.rate(now_second),
                })
                .collect();
            (
                state.startup.clone(),
                metrics,
                counters,
                state.recent_spans.iter().cloned().collect(),
            )
        })
        .unwrap_or_default();
    let overhead_samples = registry.overhead_samples.load(Ordering::Relaxed);
    let overhead_total = registry.overhead_total_ns.load(Ordering::Relaxed);

    PerformanceSnapshot {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        uptime_ms: registry.started_at.elapsed().as_millis() as u64,
        mode: mode(),
        environment: BTreeMap::from([
            ("os".to_string(), std::env::consts::OS.to_string()),
            ("arch".to_string(), std::env::consts::ARCH.to_string()),
            (
                "build".to_string(),
                if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                }
                .to_string(),
            ),
        ]),
        startup,
        resources: process_resources(registry),
        metrics,
        counters,
        recent_spans,
        instrumentation_overhead: InstrumentationOverhead {
            samples: overhead_samples,
            average_ns: overhead_total.checked_div(overhead_samples).unwrap_or(0),
            max_ns: registry.overhead_max_ns.load(Ordering::Relaxed),
        },
    }
}

pub fn write_startup_report_if_configured() {
    let Some(path) = std::env::var_os(STARTUP_REPORT_PATH_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    if !path.is_absolute() {
        return;
    }
    let Ok(encoded) = serde_json::to_vec_pretty(&snapshot()) else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    else {
        return;
    };
    let _ = std::io::Write::write_all(&mut file, &encoded);
}

impl PerformanceRegistry {
    fn record_overhead(&self, duration: Duration) {
        let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
        self.overhead_total_ns.fetch_add(nanos, Ordering::Relaxed);
        self.overhead_samples.fetch_add(1, Ordering::Relaxed);
        self.overhead_max_ns.fetch_max(nanos, Ordering::Relaxed);
    }
}

fn process_resources(registry: &PerformanceRegistry) -> ProcessResources {
    #[cfg(target_os = "linux")]
    {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let rss_bytes = std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
            .filter(|_| page_size > 0)
            .map(|pages| pages.saturating_mul(page_size as u64));
        let now = Instant::now();
        let ticks = process_cpu_ticks();
        let clock_ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let cpu_percent = ticks.and_then(|ticks| {
            let mut sample = registry.cpu_sample.lock().ok()?;
            let previous = sample.replace((now, ticks));
            let (previous_at, previous_ticks) = previous?;
            let elapsed = now.duration_since(previous_at).as_secs_f64();
            if elapsed <= 0.0 || clock_ticks <= 0 {
                return None;
            }
            Some(ticks.saturating_sub(previous_ticks) as f64 / clock_ticks as f64 / elapsed * 100.0)
        });
        ProcessResources {
            rss_bytes,
            cpu_percent,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        ProcessResources::default()
    }
}

#[cfg(target_os = "linux")]
fn process_cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields = stat
        .get(stat.rfind(')')? + 2..)?
        .split_whitespace()
        .collect::<Vec<_>>();
    let user = fields.get(11)?.parse::<u64>().ok()?;
    let system = fields.get(12)?.parse::<u64>().ok()?;
    Some(user.saturating_add(system))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_reports_tail_percentiles() {
        let mut histogram = Histogram::default();
        for duration in [1_u64, 2, 3, 4, 100] {
            histogram.record(Duration::from_millis(duration));
        }
        assert!(histogram.percentile_ms(0.50) >= 4.0);
        assert!(histogram.percentile_ms(0.95) >= 100.0);
        assert_eq!(histogram.max_us, 100_000);
    }

    #[test]
    fn profiling_retention_is_bounded() {
        let registry = PerformanceRegistry::default();
        registry
            .mode
            .store(PerformanceMode::Profiling.as_u8(), Ordering::Relaxed);
        for index in 0..(RECENT_SPAN_LIMIT + 20) {
            let mut state = registry.state.lock().unwrap();
            if state.recent_spans.len() == RECENT_SPAN_LIMIT {
                state.recent_spans.pop_front();
            }
            state.recent_spans.push_back(RecentSpan {
                name: "test".to_string(),
                category: "test".to_string(),
                duration_ms: index as f64,
                recorded_at_ms: index as u64,
                context: BTreeMap::new(),
            });
        }
        assert_eq!(
            registry.state.lock().unwrap().recent_spans.len(),
            RECENT_SPAN_LIMIT
        );
    }
}
