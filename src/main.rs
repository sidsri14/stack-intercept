pub mod embeddings;
pub mod cache;
pub mod config;
pub mod router;

use axum::{
    routing::post,
    Router,
    response::IntoResponse,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, HeaderMap, Response},
    Json,
};
use futures_util::StreamExt;
use futures_util::stream;
use reqwest::Client;
use std::net::SocketAddr;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::cache::{ExactCache, cache_key_hash, canonical_json, is_eligible};
use crate::config::ProxyConfig;
use crate::embeddings::LocalPredictor;
use crate::router::evaluate_routing;

const ALIGNMENT_BAR: f32 = 0.93;

#[derive(Clone, Debug)]
struct CacheItem {
    #[allow(dead_code)]
    prompt: String,
    vector: Vec<f32>,
    completion_response: Vec<u8>,
}

/// Build a deterministic context key for semantic safety gating.
/// Hashes everything EXCEPT the final user message, so semantically similar
/// prompts within the same conversation context can produce cache hits.
fn build_context_key(
    payload: &serde_json::Value,
    tenant_id: &Option<String>,
    upstream_base_url: &str,
) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();

    // Hash all messages EXCEPT the final user prompt
    if let Some(messages) = payload["messages"].as_array() {
        let context_messages: Vec<&serde_json::Value> = if messages.len() > 1 {
            messages[..messages.len() - 1].iter().collect()
        } else {
            vec![]
        };
        hasher.update(canonical_json(&serde_json::Value::Array(
            context_messages.into_iter().cloned().collect(),
        )).as_bytes());
    }

    let model = payload["model"].as_str().unwrap_or("unknown");
    hasher.update(model.as_bytes());
    hasher.update(upstream_base_url.as_bytes());

    if let Some(t) = tenant_id { hasher.update(t.as_bytes()); }
    if let Some(tools) = payload["tools"].as_array() {
        if !tools.is_empty() { hasher.update(canonical_json(&serde_json::Value::Array(tools.clone())).as_bytes()); }
    }
    if payload["response_format"].is_object() {
        hasher.update(canonical_json(&payload["response_format"]).as_bytes());
    }
    if !payload["tool_choice"].is_null() { hasher.update(payload["tool_choice"].to_string().as_bytes()); }
    if let Some(temp) = payload["temperature"].as_f64() { hasher.update(&temp.to_le_bytes()); }
    if let Some(tp) = payload["top_p"].as_f64() { hasher.update(&tp.to_le_bytes()); }

    format!("{:x}", hasher.finalize())
}

struct AppState {
    predictor: Option<Arc<LocalPredictor>>,
    index: RwLock<HashMap<String, Vec<CacheItem>>>,
    exact_cache: RwLock<ExactCache>,
    config: ProxyConfig,
    client: Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = ProxyConfig::from_env();
    println!("Cache mode: {:?}", config.cache_mode);

    let predictor = if config.cache_mode == config::CacheMode::Semantic {
        println!("Initializing BGE Local Embedding Weights...");
        Some(Arc::new(LocalPredictor::init_from_disk().expect("Failed to bind local model weights")))
    } else {
        println!("Skipping model load (not in semantic mode).");
        None
    };

    let shared_state = Arc::new(AppState {
        predictor,
        index: RwLock::new(HashMap::new()),
        exact_cache: RwLock::new(ExactCache::new(20000, 3600)),
        config,
        client: Client::new(),
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_intercept))
        .layer(DefaultBodyLimit::max(shared_state.config.max_body_size))
        .with_state(shared_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    println!("StackIntercept online at http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn compute_vector_dot(v1: &[f32], v2: &[f32]) -> f32 {
    v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum()
}

async fn handle_intercept(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let orig_auth = headers.get("authorization").and_then(|h| h.to_str().ok()).unwrap_or("");

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

    let tenant_id = state.config.tenant_id_header.as_ref()
        .and_then(|h| headers.get(h))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let cache_key_hash = cache_key_hash(&payload, tenant_id.clone(), &state.config.upstream_base_url);
    let context_key = build_context_key(&payload, &tenant_id, &state.config.upstream_base_url);

    // 1. Exact cache lookup (O(1) via HashMap)
    if state.config.is_cache_enabled() && !has_no_store {
        if let Some(ref key_hash) = cache_key_hash {
            let cache = state.exact_cache.read().unwrap();
            if let Some(entry) = cache.get(key_hash) {
                println!("Exact cache HIT for key {}", &key_hash[..12]);
                let cached = entry.response_body.clone();
                if is_streaming {
                    let stream = futures_util::stream::once(async move {
                        Ok::<_, std::io::Error>(Bytes::from(cached))
                    });
                    return Response::builder()
                        .header("content-type", "text/event-stream")
                        .header("x-stack-intercept", "hit")
                        .body(Body::from_stream(stream))
                        .unwrap()
                        .into_response();
                } else {
                    return Response::builder()
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

    let semantic_eligible =
        state.config.is_semantic_allowed()
        && !has_no_store
        && is_cache_eligible
        && payload["response_format"].is_null()
        && payload["tool_choice"].is_null()
        && payload["tools"].as_array().map_or(true, |a| a.is_empty());

    // 2. Offload neural computations to blocking thread (spawn_blocking)
    let target_coordinates: Option<Vec<f32>> = if semantic_eligible {
        let predictor = match state.predictor.as_ref() {
            Some(p) => Arc::clone(p),
            None => return (StatusCode::INTERNAL_SERVER_ERROR, "Predictor not initialized").into_response(),
        };
        let prompt_clone = prompt.clone();
        match tokio::task::spawn_blocking(move || predictor.encode_text(&prompt_clone)).await {
            Ok(Ok(v)) => Some(v),
            _ => return (StatusCode::INTERNAL_SERVER_ERROR, "Vector mapping failure").into_response(),
        }
    } else {
        None
    };

    // 3. Semantic cache scan (gated by context key, bucketed by HashMap)
    if semantic_eligible {
        if let Some(ref target_vec) = target_coordinates {
            let storage = state.index.read().unwrap();
            if let Some(bucket) = storage.get(&context_key) {
                for item in bucket.iter() {
                    let score = compute_vector_dot(target_vec, &item.vector);
                    if score >= ALIGNMENT_BAR {
                        println!("Semantic HIT! Similarity: {:.4}", score);
                        let cached = item.completion_response.clone();
                        if is_streaming {
                            let stream = futures_util::stream::once(async move {
                                Ok::<_, std::io::Error>(Bytes::from(cached))
                            });
                            return Response::builder()
                                .header("content-type", "text/event-stream")
                                .header("x-stack-intercept", "hit")
                                .body(Body::from_stream(stream))
                                .unwrap()
                                .into_response();
                        } else {
                            return Response::builder()
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

    // 4. Cache miss — evaluate routing and forward
    let route = evaluate_routing(
        &payload,
        &state.config.upstream_base_url,
        &state.config.fallback_base_url,
        state.config.allow_model_rewrite,
    );

    // Clone and inject the routed model into the outbound payload
    let mut modified_payload = payload.clone();
    if modified_payload["model"].as_str() != Some(&route.final_model) {
        if let Some(model_mut) = modified_payload.get_mut("model") {
            *model_mut = serde_json::Value::String(route.final_model.clone());
        }
    }

    // Pick the right auth key for the destination
    let final_auth = if route.needs_fallback_key {
        state.config.fallback_api_key.clone().unwrap_or_else(|| orig_auth.to_string())
    } else {
        orig_auth.to_string()
    };

    let upstream_res = state.client.post(&route.final_url)
        .header("authorization", final_auth)
        .json(&modified_payload)
        .send()
        .await;

    match upstream_res {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let is_success = status.is_success();

            if is_streaming {
                let prompt_clone = prompt.clone();
                let vector_clone = target_coordinates.clone();
                let state_clone = Arc::clone(&state);
                let cache_key_hash_clone = cache_key_hash.clone();
                let context_key_clone = context_key.clone();

                let request_model_clone = requested_model.clone();
                let needs_model_mask = route.needs_fallback_key;

                let raw_byte_accumulator = Arc::new(std::sync::Mutex::new(Vec::new()));
                let accumulator_clone = Arc::clone(&raw_byte_accumulator);

                // Forward chunks, rewriting model name for client SDK compatibility
                let stream = res.bytes_stream().map(move |chunk_result| {
                    match chunk_result {
                        Ok(bytes) => {
                            // Rewrite the routed model name to the originally requested model
                            // so strict client SDKs (LangChain, etc.) don't reject the stream
                            let chunk = if needs_model_mask {
                                let s = String::from_utf8_lossy(&bytes).to_string();
                                Bytes::from(s.replace("deepseek-chat", &request_model_clone))
                            } else {
                                bytes
                            };
                            if let Ok(mut buf) = raw_byte_accumulator.lock() {
                                buf.extend_from_slice(&chunk);
                            }
                            Ok(chunk)
                        },
                        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
                    }
                });

                // When stream ends, flush accumulated bytes to cache
                let stream = stream.chain(stream::once(async move {
                    let final_bytes = accumulator_clone.lock().unwrap().clone();
                    if !final_bytes.is_empty() && is_success {
                        if is_cache_eligible {
                            if let Some(ref key_hash) = cache_key_hash_clone {
                                state_clone.exact_cache.write().unwrap().insert(key_hash.clone(), final_bytes.clone());
                                println!("Stream cached (exact).");
                            }
                        }
                        if semantic_eligible {
                            if let Some(ref vector) = vector_clone {
                                let mut writer = state_clone.index.write().unwrap();
                                writer.entry(context_key_clone)
                                    .or_default()
                                    .push(CacheItem {
                                        prompt: prompt_clone.clone(),
                                        vector: vector.clone(),
                                        completion_response: final_bytes.clone(),
                                    });
                                println!("Stream cached via semantic coordinates.");
                            }
                        }
                    }
                    Ok::<_, std::io::Error>(Bytes::new())
                }));

                return Response::builder()
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
                        state.exact_cache.write().unwrap().insert(key_hash.clone(), bytes.to_vec());
                    }
                }
                if semantic_eligible {
                    if let Some(ref target_vec) = target_coordinates {
                        state.index.write().unwrap()
                            .entry(context_key.clone())
                            .or_default()
                            .push(CacheItem {
                                prompt: prompt.to_string(),
                                vector: target_vec.clone(),
                                completion_response: bytes.to_vec(),
                            });
                    }
                }
            }

            (status, res_str).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Upstream Timeout").into_response(),
    }
}
