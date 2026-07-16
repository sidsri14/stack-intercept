pub mod cache;
pub mod config;
pub mod embeddings;
pub mod router;

use crate::cache::{cache_key_hash, canonical_json, is_eligible, CacheItem, ExactCache};
use crate::cache::{evict_bucket, evict_global, load_snapshot};
use crate::config::ProxyConfig;
use crate::embeddings::LocalPredictor;
use crate::router::evaluate_routing;
use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Query, State},
    http::response::Builder as ResponseBuilder,
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use dashmap::DashMap;
use futures_util::stream;
use futures_util::StreamExt;
use reqwest::Client;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

const ALIGNMENT_BAR: f32 = 0.93;

/// Count total semantic cache entries across all DashMap buckets.
fn total_semantic_entries(index: &DashMap<String, Vec<CacheItem>>) -> usize {
    index.iter().map(|e| e.value().len()).sum()
}

/// Hashes everything EXCEPT the final user message, so semantically similar
/// prompts within the same conversation context can produce cache hits.
fn build_context_key(
    payload: &serde_json::Value,
    tenant_id: &Option<String>,
    upstream_base_url: &str,
    is_routed: bool,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    // Hash all messages EXCEPT the final user prompt
    if let Some(messages) = payload["messages"].as_array() {
        let context_messages: Vec<&serde_json::Value> = if messages.len() > 1 {
            messages[..messages.len() - 1].iter().collect()
        } else {
            vec![]
        };
        hasher.update(
            canonical_json(&serde_json::Value::Array(
                context_messages.into_iter().cloned().collect(),
            ))
            .as_bytes(),
        );
    }

    let model = payload["model"].as_str().unwrap_or("unknown");
    hasher.update(model.as_bytes());
    hasher.update(upstream_base_url.as_bytes());

    if let Some(t) = tenant_id {
        hasher.update(t.as_bytes());
    }
    // Include routing status so routed/unrouted contexts don't share semantic buckets
    if is_routed {
        hasher.update(b"routed");
    }
    if let Some(tools) = payload["tools"].as_array() {
        if !tools.is_empty() {
            hasher.update(canonical_json(&serde_json::Value::Array(tools.clone())).as_bytes());
        }
    }
    if payload["response_format"].is_object() {
        hasher.update(canonical_json(&payload["response_format"]).as_bytes());
    }
    if !payload["tool_choice"].is_null() {
        hasher.update(payload["tool_choice"].to_string().as_bytes());
    }
    if let Some(temp) = payload["temperature"].as_f64() {
        hasher.update(temp.to_le_bytes());
    }
    if let Some(tp) = payload["top_p"].as_f64() {
        hasher.update(tp.to_le_bytes());
    }

    format!("{:x}", hasher.finalize())
}

pub struct TenantMetrics {
    pub exact_hits: AtomicU64,
    pub semantic_hits: AtomicU64,
    pub misses: AtomicU64,
    pub upstream_errors: AtomicU64,
    pub routed_fallback: AtomicU64,
    pub routed_passthrough: AtomicU64,
    pub cache_inserts_exact: AtomicU64,
    pub cache_inserts_semantic: AtomicU64,
    pub reactive_failovers: AtomicU64,
}

impl TenantMetrics {
    fn new() -> Self {
        Self {
            exact_hits: AtomicU64::new(0),
            semantic_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            routed_fallback: AtomicU64::new(0),
            routed_passthrough: AtomicU64::new(0),
            cache_inserts_exact: AtomicU64::new(0),
            cache_inserts_semantic: AtomicU64::new(0),
            reactive_failovers: AtomicU64::new(0),
        }
    }
}

impl Default for TenantMetrics {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Metrics {
    pub exact_hits: AtomicU64,
    pub semantic_hits: AtomicU64,
    pub misses: AtomicU64,
    pub upstream_errors: AtomicU64,
    pub routed_fallback: AtomicU64,
    pub routed_passthrough: AtomicU64,
    pub cache_inserts_exact: AtomicU64,
    pub cache_inserts_semantic: AtomicU64,
    pub reactive_failovers: AtomicU64,
    pub started_at: std::time::Instant,
    pub tenants: DashMap<String, TenantMetrics>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            exact_hits: AtomicU64::new(0),
            semantic_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            routed_fallback: AtomicU64::new(0),
            routed_passthrough: AtomicU64::new(0),
            cache_inserts_exact: AtomicU64::new(0),
            cache_inserts_semantic: AtomicU64::new(0),
            reactive_failovers: AtomicU64::new(0),
            started_at: std::time::Instant::now(),
            tenants: DashMap::new(),
        }
    }

    pub fn tenant_key(tenant_id: &Option<String>) -> String {
        tenant_id.clone().unwrap_or_else(|| "_default_".to_string())
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

struct AppState {
    predictor: Option<Arc<LocalPredictor>>,
    index: DashMap<String, Vec<CacheItem>>,
    exact_cache: RwLock<ExactCache>,
    config: ProxyConfig,
    client: Client,
    metrics: Metrics,
    last_persist: Mutex<std::time::Instant>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = ProxyConfig::load();
    println!("Cache mode: {:?}", config.cache_mode);
    if config.admin_key.is_some() {
        println!("Admin key: configured");
    }
    if config.fallback_api_key.is_some() {
        println!("Fallback API key: configured");
    }

    let predictor = if config.cache_mode == config::CacheMode::Semantic {
        println!("Initializing BGE Local Embedding Weights...");
        Some(Arc::new(
            LocalPredictor::init_from_disk().expect("Failed to bind local model weights"),
        ))
    } else {
        println!("Skipping model load (not in semantic mode).");
        None
    };

    let shared_state = Arc::new(AppState {
        predictor,
        index: DashMap::new(),
        exact_cache: RwLock::new(ExactCache::new(
            config.exact_max_entries,
            config.exact_ttl_secs,
        )),
        config,
        client: Client::new(),
        metrics: Metrics::new(),
        last_persist: Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(10)),
    });

    // Restore cache from disk snapshot on startup
    if !shared_state.config.disable_persistence {
        if let Some(ref path) = shared_state.config.cache_path {
            match load_snapshot(path) {
                Ok(snapshot) => {
                    // Restore exact cache
                    let exact_entries: Vec<_> = snapshot
                        .exact_entries
                        .into_iter()
                        .map(|e| (e.key, e.response_body, e.created_at_epoch, e.ttl_secs))
                        .collect();
                    shared_state
                        .exact_cache
                        .write()
                        .unwrap()
                        .restore_from_snapshot(exact_entries);
                    println!(
                        "Exact cache restored: {} entries",
                        shared_state
                            .exact_cache
                            .read()
                            .unwrap()
                            .snapshot_entries()
                            .len()
                    );
                    // Restore semantic index
                    let mut semantic_count = 0usize;
                    for item in snapshot.semantic_entries {
                        let epoch_diff = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .and_then(|d| {
                                if d.as_secs() >= item.created_at_epoch {
                                    Some(std::time::Duration::from_secs(
                                        d.as_secs() - item.created_at_epoch,
                                    ))
                                } else {
                                    None
                                }
                            });
                        let created_at = match epoch_diff {
                            Some(diff) => std::time::Instant::now() - diff,
                            None => {
                                // epoch in the future — don't add
                                continue;
                            }
                        };
                        let ttl = std::time::Duration::from_secs(item.ttl_secs);
                        if created_at.elapsed() >= ttl {
                            continue; // expired
                        }
                        let cache_item = CacheItem {
                            prompt: item.prompt,
                            vector: item.vector,
                            completion_response: item.completion_response,
                            created_at,
                            ttl,
                        };
                        let mut bucket = shared_state.index.entry(item.context_key).or_default();
                        if bucket.len() < shared_state.config.semantic_max_bucket_items {
                            bucket.push(cache_item);
                            semantic_count += 1;
                        }
                    }
                    println!("Semantic cache restored: {} entries", semantic_count);
                }
                Err(e) => {
                    eprintln!("Warning: failed to load cache snapshot: {}", e);
                }
            }
        }
    }

    let server_state = shared_state.clone();
    let admin_router = Router::new()
        .route("/metrics", axum::routing::get(admin_metrics))
        .route("/cache", axum::routing::get(admin_cache_summary))
        .route("/cache", axum::routing::delete(admin_cache_flush))
        .route(
            "/cache/exact/:key",
            axum::routing::delete(admin_cache_exact_delete),
        )
        .route(
            "/cache/semantic/:context_key",
            axum::routing::delete(admin_cache_semantic_delete),
        )
        .route("/config", axum::routing::get(admin_config))
        .route(
            "/metrics/prometheus",
            axum::routing::get(admin_metrics_prometheus),
        );

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_intercept))
        .nest("/admin", admin_router)
        .layer(DefaultBodyLimit::max(shared_state.config.max_body_size))
        .with_state(shared_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    println!("StackIntercept online at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        tokio::signal::ctrl_c().await.ok();
        println!("Shutting down, flushing cache to disk...");
        let state = server_state;
        tokio::task::spawn_blocking(move || {
            flush_persistence(&state);
        })
        .await
        .ok();
        println!("Cache flushed.");
    })
    .await
    .unwrap();
}

fn as_bearer(key: &str) -> String {
    if key.starts_with("Bearer ") {
        key.to_string()
    } else {
        format!("Bearer {}", key)
    }
}

fn compute_vector_dot(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        return 0.0;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx") {
            // SAFETY: guarded by runtime AVX feature detection. The function
            // only uses unaligned loads within bounds checked by the loop.
            unsafe {
                return compute_vector_dot_avx(v1, v2);
            }
        }
    }

    compute_vector_dot_unrolled(v1, v2)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn compute_vector_dot_avx(v1: &[f32], v2: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = v1.len();
    let mut sum_reg = _mm256_setzero_ps();
    let mut i = 0usize;

    while i + 8 <= len {
        let va = _mm256_loadu_ps(v1.as_ptr().add(i));
        let vb = _mm256_loadu_ps(v2.as_ptr().add(i));
        sum_reg = _mm256_add_ps(sum_reg, _mm256_mul_ps(va, vb));
        i += 8;
    }

    let mut lanes = [0.0f32; 8];
    _mm256_storeu_ps(lanes.as_mut_ptr(), sum_reg);
    let mut total = lanes.iter().sum::<f32>();

    while i < len {
        total += v1[i] * v2[i];
        i += 1;
    }

    total
}

fn compute_vector_dot_unrolled(v1: &[f32], v2: &[f32]) -> f32 {
    let mut chunks_a = v1.chunks_exact(4);
    let mut chunks_b = v2.chunks_exact(4);
    let mut total = 0.0f32;

    for (a, b) in chunks_a.by_ref().zip(chunks_b.by_ref()) {
        total += a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    }

    total
        + chunks_a
            .remainder()
            .iter()
            .zip(chunks_b.remainder())
            .map(|(a, b)| a * b)
            .sum::<f32>()
}

/// Add transparent route headers to a response builder.
fn with_route_headers(
    builder: ResponseBuilder,
    route_label: &str,
    original_model: &str,
    routed_model: &str,
) -> ResponseBuilder {
    builder
        .header("x-stack-intercept-route", route_label)
        .header("x-stack-intercept-original-model", original_model)
        .header("x-stack-intercept-routed-model", routed_model)
}

/// Collect cache data for snapshot (shared between debounced and flush paths).
type ExactEntry = (String, Vec<u8>, u64, u64);
type SemEntry = (String, String, Vec<f32>, Vec<u8>, u64, u64);
fn collect_snapshot_data(state: &AppState) -> (Vec<ExactEntry>, Vec<SemEntry>, u64) {
    let exact_entries = state
        .exact_cache
        .read()
        .map(|c| c.snapshot_entries())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut semantic_entries: Vec<SemEntry> = Vec::new();
    for entry in state.index.iter() {
        let ctx = entry.key().clone();
        for item in entry.value().iter() {
            let elapsed = item.created_at.elapsed().as_secs();
            let epoch = now.saturating_sub(elapsed);
            semantic_entries.push((
                ctx.clone(),
                item.prompt.clone(),
                item.vector.clone(),
                item.completion_response.clone(),
                epoch,
                item.ttl.as_secs(),
            ));
        }
    }
    (exact_entries, semantic_entries, now)
}

/// Serialize and write snapshot to disk (blocking — run on spawn_blocking).
fn write_snapshot_to_disk(
    cache_path: &str,
    exact_entries: Vec<ExactEntry>,
    semantic_entries: Vec<SemEntry>,
) {
    let exact: Vec<_> = exact_entries
        .into_iter()
        .map(|(key, body, epoch, ttl)| crate::cache::SnapshotEntry {
            key,
            response_body: body,
            created_at_epoch: epoch,
            ttl_secs: ttl,
        })
        .collect();
    let semantic: Vec<_> = semantic_entries
        .into_iter()
        .map(
            |(ctx, prompt, vector, body, epoch, ttl)| crate::cache::SnapshotItem {
                context_key: ctx,
                prompt,
                vector,
                completion_response: body,
                created_at_epoch: epoch,
                ttl_secs: ttl,
            },
        )
        .collect();
    let snapshot = crate::cache::Snapshot {
        exact_entries: exact,
        semantic_entries: semantic,
    };
    match rmp_serde::to_vec(&snapshot) {
        Ok(bytes) => {
            let tmp_path = format!("{}.tmp", cache_path);
            if std::fs::write(&tmp_path, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp_path, cache_path);
            }
        }
        Err(e) => eprintln!("Snapshot serialization failed: {}", e),
    }
}

/// Persist cache state to disk with debounce (at most once per second).
fn persist_after_insert(state: &AppState) {
    if state.config.disable_persistence {
        return;
    }
    let cache_path = match &state.config.cache_path {
        Some(p) => p.clone(),
        None => return,
    };
    // Debounce: at most one write per second
    let mut last = state.last_persist.lock().unwrap();
    if last.elapsed() < std::time::Duration::from_secs(1) {
        return;
    }
    *last = std::time::Instant::now();
    drop(last);
    // Collect data synchronously, spawn blocking I/O
    let (exact_entries, semantic_entries, _now) = collect_snapshot_data(state);
    tokio::task::spawn_blocking(move || {
        write_snapshot_to_disk(&cache_path, exact_entries, semantic_entries);
    });
}

/// Force a synchronous flush to disk (ignores debounce). Used on shutdown.
fn flush_persistence(state: &AppState) {
    if state.config.disable_persistence {
        return;
    }
    let cache_path = match &state.config.cache_path {
        Some(p) => p.clone(),
        None => return,
    };
    let (exact_entries, semantic_entries, _now) = collect_snapshot_data(state);
    write_snapshot_to_disk(&cache_path, exact_entries, semantic_entries);
}

/// Check if a peer address is a loopback address (127.0.0.1 or ::1).
fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Check admin auth. Returns Ok(()) or a 403 StatusCode.
fn check_admin_auth(
    headers: &HeaderMap,
    addr: SocketAddr,
    config: &ProxyConfig,
) -> Result<(), StatusCode> {
    // If admin_key is set, it's always required
    if let Some(ref key) = config.admin_key {
        let provided = headers
            .get("x-admin-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != key {
            return Err(StatusCode::FORBIDDEN);
        }
        return Ok(());
    }
    // No admin key: only allow loopback peers
    if !is_loopback(&addr) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

#[derive(Serialize)]
struct GlobalMetricsResponse {
    exact_hits: u64,
    semantic_hits: u64,
    misses: u64,
    upstream_errors: u64,
    routed_fallback: u64,
    routed_passthrough: u64,
    cache_inserts_exact: u64,
    cache_inserts_semantic: u64,
    reactive_failovers: u64,
}

#[derive(Serialize)]
struct TenantMetricsSnapshot {
    exact_hits: u64,
    semantic_hits: u64,
    misses: u64,
    upstream_errors: u64,
    routed_fallback: u64,
    routed_passthrough: u64,
    cache_inserts_exact: u64,
    cache_inserts_semantic: u64,
    reactive_failovers: u64,
}

#[derive(Serialize)]
struct MetricsResponse {
    uptime_secs: u64,
    global: GlobalMetricsResponse,
    tenants: std::collections::HashMap<String, TenantMetricsSnapshot>,
}

#[derive(Serialize)]
struct CacheSummaryExact {
    entries: usize,
    max_entries: usize,
    ttl_secs: u64,
}

#[derive(Serialize)]
struct CacheSummarySemantic {
    buckets: usize,
    entries: usize,
    max_items: usize,
    max_bucket_items: usize,
    ttl_secs: u64,
}

#[derive(Serialize)]
struct CacheSummaryResponse {
    exact: CacheSummaryExact,
    semantic: CacheSummarySemantic,
}

async fn admin_metrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }
    let m = &state.metrics;
    let global = GlobalMetricsResponse {
        exact_hits: m.exact_hits.load(Ordering::Relaxed),
        semantic_hits: m.semantic_hits.load(Ordering::Relaxed),
        misses: m.misses.load(Ordering::Relaxed),
        upstream_errors: m.upstream_errors.load(Ordering::Relaxed),
        routed_fallback: m.routed_fallback.load(Ordering::Relaxed),
        routed_passthrough: m.routed_passthrough.load(Ordering::Relaxed),
        cache_inserts_exact: m.cache_inserts_exact.load(Ordering::Relaxed),
        cache_inserts_semantic: m.cache_inserts_semantic.load(Ordering::Relaxed),
        reactive_failovers: m.reactive_failovers.load(Ordering::Relaxed),
    };
    let mut tenants = std::collections::HashMap::new();
    for entry in m.tenants.iter() {
        let snapshot = TenantMetricsSnapshot {
            exact_hits: entry.value().exact_hits.load(Ordering::Relaxed),
            semantic_hits: entry.value().semantic_hits.load(Ordering::Relaxed),
            misses: entry.value().misses.load(Ordering::Relaxed),
            upstream_errors: entry.value().upstream_errors.load(Ordering::Relaxed),
            routed_fallback: entry.value().routed_fallback.load(Ordering::Relaxed),
            routed_passthrough: entry.value().routed_passthrough.load(Ordering::Relaxed),
            cache_inserts_exact: entry.value().cache_inserts_exact.load(Ordering::Relaxed),
            cache_inserts_semantic: entry.value().cache_inserts_semantic.load(Ordering::Relaxed),
            reactive_failovers: entry.value().reactive_failovers.load(Ordering::Relaxed),
        };
        tenants.insert(entry.key().clone(), snapshot);
    }
    let resp = MetricsResponse {
        uptime_secs: m.started_at.elapsed().as_secs(),
        global,
        tenants,
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn admin_metrics_prometheus(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, "unauthorized").into_response();
    }

    let m = &state.metrics;
    let mut body = String::new();

    // Helper: emit a HELP/TYPE header + one line per tenant
    fn emit_metric(
        body: &mut String,
        name: &str,
        help: &str,
        global_val: u64,
        tenants: &DashMap<String, TenantMetrics>,
        getter: fn(&TenantMetrics) -> u64,
    ) {
        body.push_str(&format!("# HELP {} {}\n", name, help));
        body.push_str(&format!("# TYPE {} counter\n", name));
        body.push_str(&format!("{}{{tenant=\"\"}} {}\n", name, global_val));
        for entry in tenants.iter() {
            let val = getter(entry.value());
            body.push_str(&format!("{}{{tenant=\"{}\"}} {}\n", name, entry.key(), val));
        }
        body.push('\n');
    }

    emit_metric(
        &mut body,
        "stack_intercept_exact_hits",
        "Total exact cache hits",
        m.exact_hits.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.exact_hits.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_semantic_hits",
        "Total semantic cache hits",
        m.semantic_hits.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.semantic_hits.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_misses",
        "Total cache misses",
        m.misses.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.misses.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_upstream_errors",
        "Total upstream connection errors",
        m.upstream_errors.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.upstream_errors.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_routed_fallback",
        "Total requests routed to fallback provider",
        m.routed_fallback.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.routed_fallback.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_routed_passthrough",
        "Total requests passed through to upstream",
        m.routed_passthrough.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.routed_passthrough.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_cache_inserts_exact",
        "Total exact cache inserts",
        m.cache_inserts_exact.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.cache_inserts_exact.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_cache_inserts_semantic",
        "Total semantic cache inserts",
        m.cache_inserts_semantic.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.cache_inserts_semantic.load(Ordering::Relaxed),
    );
    emit_metric(
        &mut body,
        "stack_intercept_reactive_failovers",
        "Total reactive failover events",
        m.reactive_failovers.load(Ordering::Relaxed),
        &m.tenants,
        |t| t.reactive_failovers.load(Ordering::Relaxed),
    );

    // Uptime gauge
    body.push_str("# HELP stack_intercept_uptime_seconds Server uptime in seconds\n");
    body.push_str("# TYPE stack_intercept_uptime_seconds gauge\n");
    body.push_str(&format!(
        "stack_intercept_uptime_seconds {}\n",
        m.started_at.elapsed().as_secs()
    ));

    Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .status(StatusCode::OK)
        .body(Body::from(body))
        .unwrap()
        .into_response()
}

async fn admin_cache_summary(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }
    let exact = state.exact_cache.read().unwrap();
    let semantic_buckets = state.index.len();
    let semantic_entries: usize = state.index.iter().map(|e| e.value().len()).sum();
    let resp = CacheSummaryResponse {
        exact: CacheSummaryExact {
            entries: exact.len(),
            max_entries: exact.max_entries(),
            ttl_secs: exact.default_ttl_secs(),
        },
        semantic: CacheSummarySemantic {
            buckets: semantic_buckets,
            entries: semantic_entries,
            max_items: state.config.semantic_max_items,
            max_bucket_items: state.config.semantic_max_bucket_items,
            ttl_secs: state.config.semantic_ttl_secs,
        },
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn admin_cache_flush(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }
    // Clear exact cache
    state.exact_cache.write().unwrap().clear();
    // Clear semantic index
    state.index.clear();
    // Force write empty snapshot to disk
    if !state.config.disable_persistence {
        if let Some(ref path) = state.config.cache_path {
            let empty_snapshot = crate::cache::Snapshot {
                exact_entries: vec![],
                semantic_entries: vec![],
            };
            match rmp_serde::to_vec(&empty_snapshot) {
                Ok(bytes) => {
                    let tmp_path = format!("{}.tmp", path);
                    if std::fs::write(&tmp_path, &bytes).is_ok() {
                        if std::fs::rename(&tmp_path, path).is_err() {
                            eprintln!("Failed to rename empty snapshot to {}", path);
                        }
                    } else {
                        eprintln!("Failed to write empty snapshot to {}", tmp_path);
                    }
                }
                Err(e) => eprintln!("Failed to serialize empty snapshot: {}", e),
            }
        }
    }
    let exact_guard = state.exact_cache.read().unwrap();
    let resp = CacheSummaryResponse {
        exact: CacheSummaryExact {
            entries: 0,
            max_entries: exact_guard.max_entries(),
            ttl_secs: exact_guard.default_ttl_secs(),
        },
        semantic: CacheSummarySemantic {
            buckets: 0,
            entries: 0,
            max_items: state.config.semantic_max_items,
            max_bucket_items: state.config.semantic_max_bucket_items,
            ttl_secs: state.config.semantic_ttl_secs,
        },
    };
    (StatusCode::OK, Json(resp)).into_response()
}

async fn admin_cache_exact_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }
    let removed = state.exact_cache.write().unwrap().remove(&key);
    (
        StatusCode::OK,
        Json(serde_json::json!({"removed": removed})),
    )
        .into_response()
}

async fn admin_cache_semantic_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::Path(context_key): axum::extract::Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    // If item_id query param is present, evict a single item from the bucket
    if let Some(item_id_str) = params.get("item_id") {
        let item_id: usize = match item_id_str.parse() {
            Ok(id) => id,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid item_id"})),
                )
                    .into_response();
            }
        };
        let removed = cache::evict_item(&state.index, &context_key, item_id);
        return (
            StatusCode::OK,
            Json(serde_json::json!({"removed": removed})),
        )
            .into_response();
    }

    // No item_id: remove the entire bucket
    let existed = state.index.remove(&context_key).is_some();
    (
        StatusCode::OK,
        Json(serde_json::json!({"removed": existed})),
    )
        .into_response()
}

async fn admin_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    let cache_mode_str = match state.config.cache_mode {
        config::CacheMode::Off => "off",
        config::CacheMode::Exact => "exact",
        config::CacheMode::Semantic => "semantic",
    };

    let admin_key = state.config.admin_key.as_deref().map(|_| "********");

    let fallback_api_key = state.config.fallback_api_key.as_ref().map(|k| {
        if k.len() > 4 {
            format!("{}*****", &k[..4])
        } else {
            "*****".to_string()
        }
    });

    let resp = serde_json::json!({
        "cache_mode": cache_mode_str,
        "tenant_id_header": state.config.tenant_id_header,
        "allow_model_rewrite": state.config.allow_model_rewrite,
        "upstream_base_url": state.config.upstream_base_url,
        "fallback_base_url": state.config.fallback_base_url,
        "fallback_api_key": fallback_api_key,
        "admin_key": admin_key,
        "exact_max_entries": state.config.exact_max_entries,
        "exact_ttl_secs": state.config.exact_ttl_secs,
        "semantic_max_items": state.config.semantic_max_items,
        "semantic_max_bucket_items": state.config.semantic_max_bucket_items,
        "semantic_ttl_secs": state.config.semantic_ttl_secs,
        "cache_path": state.config.cache_path,
        "disable_persistence": state.config.disable_persistence,
        "max_body_size": state.config.max_body_size,
        "reactive_failover": state.config.reactive_failover,
        "failover_model": state.config.failover_model,
        "failover_status_codes": state.config.failover_status_codes,
    });
    (StatusCode::OK, Json(resp)).into_response()
}

async fn handle_intercept(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let orig_auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let prompt = payload["messages"]
        .as_array()
        .and_then(|msg| msg.last())
        .and_then(|last_msg| last_msg["content"].as_str())
        .unwrap_or("")
        .to_string();

    let is_streaming = payload["stream"].as_bool().unwrap_or(false);
    let has_no_store = payload["cache_control"].as_str() == Some("no_store");

    // Capture the originally requested model BEFORE routing may rewrite it
    let requested_model = payload["model"].as_str().unwrap_or("unknown").to_string();

    let tenant_id = state
        .config
        .tenant_id_header
        .as_ref()
        .and_then(|h| headers.get(h))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Check x-stack-intercept-no-route header (allows per-request opt-out)
    let no_route = headers
        .get("x-stack-intercept-no-route")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Check x-stack-intercept-no-semantic-cache header (allows per-request
    // semantic cache opt-out while still using exact cache)
    let no_semantic_cache = headers
        .get("x-stack-intercept-no-semantic-cache")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    // Evaluate routing BEFORE cache lookup so the cache key is namespace-aware
    let mut route = evaluate_routing(
        &payload,
        &state.config.upstream_base_url,
        &state.config.fallback_base_url,
        state.config.allow_model_rewrite,
        no_route,
    );

    // Safety: if routing chose fallback but no fallback API key is configured,
    // force passthrough to prevent leaking the original provider's API key
    // to an unknown downstream.
    if route.needs_fallback_key && state.config.fallback_api_key.is_none() {
        println!(
            "Route blocked (no fallback key configured): {} stays on upstream.",
            requested_model
        );
        route = router::RouteDecision {
            final_url: format!("{}/v1/chat/completions", state.config.upstream_base_url),
            final_model: requested_model.clone(),
            needs_fallback_key: false,
        };
    }

    let mut route_label = if route.needs_fallback_key {
        "fallback"
    } else {
        "passthrough"
    };
    let mut routed_model = route.final_model.clone();

    if route.needs_fallback_key {
        state
            .metrics
            .routed_fallback
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .tenants
            .entry(Metrics::tenant_key(&tenant_id))
            .or_default()
            .routed_fallback
            .fetch_add(1, Ordering::Relaxed);
    } else {
        state
            .metrics
            .routed_passthrough
            .fetch_add(1, Ordering::Relaxed);
        state
            .metrics
            .tenants
            .entry(Metrics::tenant_key(&tenant_id))
            .or_default()
            .routed_passthrough
            .fetch_add(1, Ordering::Relaxed);
    }

    // Build routing namespace for cache key isolation (always present —
    // passthrough and fallback each get a unique, versioned namespace so
    // future routing policy changes don't accidentally share cache keys).
    let routing_namespace =
        route.cache_namespace(&state.config.upstream_base_url, &requested_model);

    let cache_key_hash = cache_key_hash(
        &payload,
        tenant_id.clone(),
        &state.config.upstream_base_url,
        Some(&routing_namespace),
    );
    let context_key = build_context_key(
        &payload,
        &tenant_id,
        &state.config.upstream_base_url,
        route.needs_fallback_key,
    );

    // 1. Exact cache lookup (O(1) via HashMap)
    if state.config.is_cache_enabled() && !has_no_store {
        if let Some(ref key_hash) = cache_key_hash {
            let cache = state.exact_cache.read().unwrap();
            if let Some(entry) = cache.get(key_hash) {
                println!("Exact cache HIT for key {}", &key_hash[..12]);
                state.metrics.exact_hits.fetch_add(1, Ordering::Relaxed);
                state
                    .metrics
                    .tenants
                    .entry(Metrics::tenant_key(&tenant_id))
                    .or_default()
                    .exact_hits
                    .fetch_add(1, Ordering::Relaxed);
                let cached = entry.response_body.clone();
                if is_streaming {
                    let stream = futures_util::stream::once(async move {
                        Ok::<_, std::io::Error>(Bytes::from(cached))
                    });
                    return with_route_headers(
                        Response::builder(),
                        route_label,
                        &requested_model,
                        &routed_model,
                    )
                    .header("content-type", "text/event-stream")
                    .header("x-stack-intercept", "hit")
                    .body(Body::from_stream(stream))
                    .unwrap()
                    .into_response();
                } else {
                    return with_route_headers(
                        Response::builder(),
                        route_label,
                        &requested_model,
                        &routed_model,
                    )
                    .header("content-type", "application/json")
                    .header("x-stack-intercept", "hit")
                    .body(Body::from(cached))
                    .unwrap()
                    .into_response();
                }
            }
        }
    }

    let is_cache_eligible = is_eligible(&payload);

    let semantic_eligible = state.config.is_semantic_allowed()
        && !no_semantic_cache
        && !has_no_store
        && is_cache_eligible
        && payload["response_format"].is_null()
        && payload["tool_choice"].is_null()
        && payload["tools"].as_array().is_none_or(|a| a.is_empty());

    // 2. Offload neural computations to blocking thread (spawn_blocking)
    let target_coordinates: Option<Vec<f32>> = if semantic_eligible {
        let predictor = match state.predictor.as_ref() {
            Some(p) => Arc::clone(p),
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Predictor not initialized",
                )
                    .into_response()
            }
        };
        let prompt_clone = prompt.clone();
        match tokio::task::spawn_blocking(move || predictor.encode_text(&prompt_clone)).await {
            Ok(Ok(v)) => Some(v),
            _ => {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Vector mapping failure")
                    .into_response()
            }
        }
    } else {
        None
    };

    // 3. Semantic cache scan (gated by context key, bucketed by DashMap)
    if semantic_eligible {
        if let Some(ref target_vec) = target_coordinates {
            if let Some(bucket) = state.index.get(&context_key) {
                for item in bucket.iter() {
                    let score = compute_vector_dot(target_vec, &item.vector);
                    if score >= ALIGNMENT_BAR {
                        println!("Semantic HIT! Similarity: {:.4}", score);
                        state.metrics.semantic_hits.fetch_add(1, Ordering::Relaxed);
                        state
                            .metrics
                            .tenants
                            .entry(Metrics::tenant_key(&tenant_id))
                            .or_default()
                            .semantic_hits
                            .fetch_add(1, Ordering::Relaxed);
                        let cached = item.completion_response.clone();
                        if is_streaming {
                            let stream = futures_util::stream::once(async move {
                                Ok::<_, std::io::Error>(Bytes::from(cached))
                            });
                            return with_route_headers(
                                Response::builder(),
                                route_label,
                                &requested_model,
                                &routed_model,
                            )
                            .header("content-type", "text/event-stream")
                            .header("x-stack-intercept", "hit")
                            .body(Body::from_stream(stream))
                            .unwrap()
                            .into_response();
                        } else {
                            return with_route_headers(
                                Response::builder(),
                                route_label,
                                &requested_model,
                                &routed_model,
                            )
                            .header("content-type", "application/json")
                            .header("x-stack-intercept", "hit")
                            .body(Body::from(cached))
                            .unwrap()
                            .into_response();
                        }
                    }
                }
            }
        }
    }

    // 4. Cache miss — forward using the route decision
    // Clone and inject the routed model into the outbound payload
    let mut modified_payload = payload.clone();
    if modified_payload["model"].as_str() != Some(&routed_model) {
        if let Some(model_mut) = modified_payload.get_mut("model") {
            *model_mut = serde_json::Value::String(routed_model.clone());
        }
    }

    // Pick the right auth key for the destination
    let mut final_auth = if route.needs_fallback_key {
        as_bearer(
            state
                .config
                .fallback_api_key
                .as_ref()
                .expect("fallback route requires fallback_api_key"),
        )
    } else {
        orig_auth.to_string()
    };

    state.metrics.misses.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .tenants
        .entry(Metrics::tenant_key(&tenant_id))
        .or_default()
        .misses
        .fetch_add(1, Ordering::Relaxed);

    let mut final_url = route.final_url.clone();
    let mut upstream_res = state
        .client
        .post(&final_url)
        .header("authorization", &final_auth)
        .json(&modified_payload)
        .send()
        .await;

    // Check if we should failover:
    // Failover is triggered if:
    // 1. STACK_INTERCEPT_REACTIVE_FAILOVER is enabled
    // 2. We are not already using the fallback (needs_fallback_key is false)
    // 3. A fallback API key is configured
    // 4. The upstream request failed (Err) OR returned a status code in failover_status_codes
    let should_failover = state.config.reactive_failover
        && !route.needs_fallback_key
        && state.config.fallback_api_key.is_some();

    if should_failover {
        let is_failed = match &upstream_res {
            Err(_) => true,
            Ok(res) => state
                .config
                .failover_status_codes
                .contains(&res.status().as_u16()),
        };

        if is_failed {
            println!(
                "Reactive failover triggered! Upstream request failed (result: {:?}). Routing to fallback...",
                upstream_res.as_ref().map(|r| r.status())
            );
            state
                .metrics
                .reactive_failovers
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .tenants
                .entry(Metrics::tenant_key(&tenant_id))
                .or_default()
                .reactive_failovers
                .fetch_add(1, Ordering::Relaxed);

            // Rewrite final_url to use fallback
            final_url = format!("{}/v1/chat/completions", state.config.fallback_base_url);

            // Rewrite final_auth to use fallback API key
            final_auth = as_bearer(state.config.fallback_api_key.as_ref().unwrap());

            // Rewrite payload model name if failover_model is configured
            if let Some(ref fm) = state.config.failover_model {
                routed_model = fm.clone();
                if let Some(model_mut) = modified_payload.get_mut("model") {
                    *model_mut = serde_json::Value::String(fm.clone());
                }
            }

            route_label = "fallback";

            // Retry the request to the fallback provider
            upstream_res = state
                .client
                .post(&final_url)
                .header("authorization", &final_auth)
                .json(&modified_payload)
                .send()
                .await;
        }
    }

    match upstream_res {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let is_success = status.is_success();

            if is_streaming {
                let prompt_clone = prompt.clone();
                let vector_clone = target_coordinates.clone();
                let state_clone = Arc::clone(&state);
                let cache_key_hash_clone = cache_key_hash.clone();
                let context_key_clone = context_key.clone();
                let tenant_id_clone = tenant_id.clone();

                let raw_byte_accumulator = Arc::new(std::sync::Mutex::new(Vec::new()));
                let accumulator_clone = Arc::clone(&raw_byte_accumulator);

                // Forward chunks transparently — no model-name masking.
                // Route headers inform the client of the actual provider.
                let stream = res
                    .bytes_stream()
                    .map(move |chunk_result| match chunk_result {
                        Ok(bytes) => {
                            if let Ok(mut buf) = raw_byte_accumulator.lock() {
                                buf.extend_from_slice(&bytes);
                            }
                            Ok(bytes)
                        }
                        Err(e) => {
                            let error_frame = format!(
                                "data: {}\n\ndata: [DONE]\n\n",
                                serde_json::json!({"error": {"message": format!("Upstream stream error: {}", e)}})
                            );
                            Ok(Bytes::from(error_frame))
                        }
                    });

                // When stream ends, flush accumulated bytes to cache
                let stream = stream.chain(stream::once(async move {
                    let final_bytes = accumulator_clone.lock().unwrap().clone();
                    if !final_bytes.is_empty() && is_success {
                        if is_cache_eligible {
                            if let Some(ref key_hash) = cache_key_hash_clone {
                                state_clone
                                    .exact_cache
                                    .write()
                                    .unwrap()
                                    .insert(key_hash.clone(), final_bytes.clone());
                                state_clone
                                    .metrics
                                    .cache_inserts_exact
                                    .fetch_add(1, Ordering::Relaxed);
                                state_clone
                                    .metrics
                                    .tenants
                                    .entry(Metrics::tenant_key(&tenant_id_clone))
                                    .or_default()
                                    .cache_inserts_exact
                                    .fetch_add(1, Ordering::Relaxed);
                                println!("Stream cached (exact).");
                                persist_after_insert(&state_clone);
                            }
                        }
                        if semantic_eligible {
                            if let Some(ref vector) = vector_clone {
                                let mut bucket =
                                    state_clone.index.entry(context_key_clone).or_default();
                                let item = CacheItem {
                                    prompt: prompt_clone.clone(),
                                    vector: vector.clone(),
                                    completion_response: final_bytes.clone(),
                                    created_at: std::time::Instant::now(),
                                    ttl: std::time::Duration::from_secs(
                                        state_clone.config.semantic_ttl_secs,
                                    ),
                                };
                                // Push first, then evict — avoids max+1 off-by-one
                                bucket.push(item);
                                state_clone
                                    .metrics
                                    .cache_inserts_semantic
                                    .fetch_add(1, Ordering::Relaxed);
                                state_clone
                                    .metrics
                                    .tenants
                                    .entry(Metrics::tenant_key(&tenant_id_clone))
                                    .or_default()
                                    .cache_inserts_semantic
                                    .fetch_add(1, Ordering::Relaxed);
                                evict_bucket(
                                    &mut bucket,
                                    state_clone.config.semantic_max_bucket_items,
                                );
                                if total_semantic_entries(&state_clone.index)
                                    > state_clone.config.semantic_max_items
                                {
                                    evict_global(
                                        &state_clone.index,
                                        state_clone.config.semantic_max_items,
                                    );
                                }
                                persist_after_insert(&state_clone);
                                println!("Stream cached via semantic coordinates.");
                            }
                        }
                    }
                    Ok::<_, std::io::Error>(Bytes::new())
                }));

                return with_route_headers(
                    Response::builder(),
                    route_label,
                    &requested_model,
                    &routed_model,
                )
                .status(status)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-store")
                .header("x-stack-intercept", "miss")
                .body(Body::from_stream(stream))
                .unwrap()
                .into_response();
            }

            // Non-streaming path
            let bytes = res.bytes().await.unwrap_or_default();
            let res_str = String::from_utf8_lossy(&bytes).to_string();

            if is_success {
                if is_cache_eligible {
                    if let Some(ref key_hash) = cache_key_hash {
                        state
                            .exact_cache
                            .write()
                            .unwrap()
                            .insert(key_hash.clone(), bytes.to_vec());
                        state
                            .metrics
                            .cache_inserts_exact
                            .fetch_add(1, Ordering::Relaxed);
                        state
                            .metrics
                            .tenants
                            .entry(Metrics::tenant_key(&tenant_id))
                            .or_default()
                            .cache_inserts_exact
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    persist_after_insert(&state);
                }
                if semantic_eligible {
                    if let Some(ref target_vec) = target_coordinates {
                        let item = CacheItem {
                            prompt: prompt.to_string(),
                            vector: target_vec.clone(),
                            completion_response: bytes.to_vec(),
                            created_at: std::time::Instant::now(),
                            ttl: std::time::Duration::from_secs(state.config.semantic_ttl_secs),
                        };
                        let mut bucket = state.index.entry(context_key.clone()).or_default();
                        // Push first, then evict — avoids max+1 off-by-one
                        bucket.push(item);
                        state
                            .metrics
                            .cache_inserts_semantic
                            .fetch_add(1, Ordering::Relaxed);
                        state
                            .metrics
                            .tenants
                            .entry(Metrics::tenant_key(&tenant_id))
                            .or_default()
                            .cache_inserts_semantic
                            .fetch_add(1, Ordering::Relaxed);
                        evict_bucket(&mut bucket, state.config.semantic_max_bucket_items);
                        if total_semantic_entries(&state.index) > state.config.semantic_max_items {
                            evict_global(&state.index, state.config.semantic_max_items);
                        }
                        persist_after_insert(&state);
                    }
                }
            }

            with_route_headers(
                Response::builder(),
                route_label,
                &requested_model,
                &routed_model,
            )
            .status(status)
            .header("x-stack-intercept", "miss")
            .body(Body::from(res_str))
            .unwrap()
            .into_response()
        }
        Err(_) => {
            state
                .metrics
                .upstream_errors
                .fetch_add(1, Ordering::Relaxed);
            state
                .metrics
                .tenants
                .entry(Metrics::tenant_key(&tenant_id))
                .or_default()
                .upstream_errors
                .fetch_add(1, Ordering::Relaxed);
            let body: Body = if is_streaming {
                Body::from(format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    serde_json::json!({"error": {"message": "Upstream Timeout"}})
                ))
            } else {
                Body::from("Upstream Timeout")
            };
            let mut builder = with_route_headers(
                Response::builder(),
                route_label,
                &requested_model,
                &routed_model,
            )
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("x-stack-intercept", "error");
            if is_streaming {
                builder = builder.header("content-type", "text/event-stream");
            }
            builder.body(body).unwrap().into_response()
        }
    }
}

#[cfg(test)]
mod admin_auth_tests {
    use super::*;
    use crate::config::ProxyConfig;

    #[test]
    fn test_compute_vector_dot_matches_expected() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let b = vec![0.5, -1.0, 2.0, 0.25, 3.0, -0.5, 1.5, 2.0, -2.0];
        let expected = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>();

        assert!((compute_vector_dot(&a, &b) - expected).abs() < 1e-5);
    }

    #[test]
    fn test_compute_vector_dot_empty_and_mismatched() {
        assert_eq!(compute_vector_dot(&[], &[]), 0.0);
        assert_eq!(compute_vector_dot(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn test_is_loopback_true() {
        let addr4 = SocketAddr::from(([127, 0, 0, 1], 8080));
        assert!(is_loopback(&addr4));
        let addr6 = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 8080));
        assert!(is_loopback(&addr6));
    }

    #[test]
    fn test_is_loopback_false() {
        let addr = SocketAddr::from(([192, 168, 1, 1], 8080));
        assert!(!is_loopback(&addr));
        let addr2 = SocketAddr::from(([10, 0, 0, 1], 8080));
        assert!(!is_loopback(&addr2));
    }

    // Test 5: Admin auth localhost allowed without key
    #[test]
    fn test_admin_auth_loopback_allowed_no_key() {
        let cfg = ProxyConfig::defaults(); // admin_key = None
        let headers = HeaderMap::new();
        let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert!(
            result.is_ok(),
            "loopback without key should be allowed, got: {:?}",
            result
        );
    }

    // Test 6: Admin auth non-loopback forbidden without key
    #[test]
    fn test_admin_auth_non_loopback_forbidden_no_key() {
        let cfg = ProxyConfig::defaults();
        let headers = HeaderMap::new();
        let addr = SocketAddr::from(([192, 168, 1, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert_eq!(result, Err(StatusCode::FORBIDDEN));
    }

    // Test 7: Admin auth key behavior
    #[test]
    fn test_admin_auth_correct_key_non_loopback() {
        let mut cfg = ProxyConfig::defaults();
        cfg.admin_key = Some("secret123".to_string());
        let mut headers = HeaderMap::new();
        headers.insert("x-admin-key", "secret123".parse().unwrap());
        let addr = SocketAddr::from(([10, 0, 0, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert!(
            result.is_ok(),
            "correct key on non-loopback should be allowed, got: {:?}",
            result
        );
    }

    #[test]
    fn test_admin_auth_wrong_key_non_loopback() {
        let mut cfg = ProxyConfig::defaults();
        cfg.admin_key = Some("secret123".to_string());
        let mut headers = HeaderMap::new();
        headers.insert("x-admin-key", "wrong".parse().unwrap());
        let addr = SocketAddr::from(([10, 0, 0, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert_eq!(result, Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn test_admin_auth_missing_key_non_loopback() {
        let mut cfg = ProxyConfig::defaults();
        cfg.admin_key = Some("secret123".to_string());
        let headers = HeaderMap::new();
        let addr = SocketAddr::from(([10, 0, 0, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert_eq!(result, Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn test_admin_auth_correct_key_loopback() {
        let mut cfg = ProxyConfig::defaults();
        cfg.admin_key = Some("secret123".to_string());
        let mut headers = HeaderMap::new();
        headers.insert("x-admin-key", "secret123".parse().unwrap());
        let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
        let result = check_admin_auth(&headers, addr, &cfg);
        assert!(
            result.is_ok(),
            "correct key on loopback should be allowed, got: {:?}",
            result
        );
    }
}
