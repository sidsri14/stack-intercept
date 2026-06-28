pub mod embeddings;
pub mod cache;
pub mod config;

use axum::{
    routing::post,
    Router,
    response::IntoResponse,
    body::{Body, Bytes},
    http::{StatusCode, HeaderMap, Response},
    Json,
    extract::State,
};
use futures_util::StreamExt;
use futures_util::stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use crate::cache::{CacheKey, ExactCache};
use crate::config::ProxyConfig;
use crate::embeddings::LocalPredictor;

const ALIGNMENT_BAR: f32 = 0.93;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CacheItem {
    prompt: String,
    vector: Vec<f32>,
    completion_response: String,
    context_key: String, // hash of system_prompt + model + tenant_id for safety gating
}

/// Build a deterministic context key for semantic safety gating.
/// Includes system prompt, model, and tenant — all dimensions that must match
/// before a semantic cache hit is allowed.
fn build_context_key(payload: &serde_json::Value, tenant_id: &Option<String>) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    // System prompt
    let system_prompt = payload["messages"].as_array()
        .and_then(|msgs| msgs.iter().find(|m| m["role"] == "system"))
        .and_then(|m| m["content"].as_str())
        .unwrap_or("");
    hasher.update(system_prompt.as_bytes());
    // Model
    let model = payload["model"].as_str().unwrap_or("unknown");
    hasher.update(model.as_bytes());
    // Tenant
    if let Some(t) = tenant_id {
        hasher.update(t.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

struct AppState {
    predictor: LocalPredictor,
    index: RwLock<Vec<CacheItem>>,
    exact_cache: RwLock<ExactCache>,
    config: ProxyConfig,
    client: Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = ProxyConfig::from_env();
    println!("Cache mode: {:?}", config.cache_mode);

    println!("Initializing BGE Local Embedding Weights...");
    let predictor = LocalPredictor::init_from_disk().expect("Failed to bind local model weights");

    let shared_state = Arc::new(AppState {
        predictor,
        index: RwLock::new(Vec::new()),
        exact_cache: RwLock::new(ExactCache::new(10000, 3600)),
        config,
        client: Client::new(),
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_intercept))
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
    let orig_auth = headers.get("authorization")
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

    let tenant_id = state.config.tenant_id_header.as_ref()
        .and_then(|h| headers.get(h))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Build cache key (used for both exact lookup and eligibility checks)
    let cache_key = CacheKey::from_payload(&payload, tenant_id.clone());
    let cache_key_hash = cache_key.as_ref().map(|k| k.hash());

    // Build context key for semantic safety gating
    let context_key = build_context_key(&payload, &tenant_id);

    // Exact cache lookup (gated by config and no_store)
    if state.config.is_cache_enabled() && !has_no_store {
        if let Some(ref key_hash) = cache_key_hash {
            let cache = state.exact_cache.read().unwrap();
            if let Some(entry) = cache.get(key_hash) {
                println!("Exact cache HIT for key {}", &key_hash[..12]);
                if is_streaming {
                    let cached = entry.response_body.clone();
                    let stream = futures_util::stream::once(async move {
                        Ok::<_, std::io::Error>(Bytes::from(cached))
                    });
                    let body = Body::from_stream(stream);
                    return Response::builder()
                        .header("content-type", "text/event-stream")
                        .header("x-stack-intercept", "hit")
                        .body(body)
                        .unwrap()
                        .into_response();
                } else {
                    return Response::builder()
                        .header("content-type", "application/json")
                        .header("x-stack-intercept", "hit")
                        .body(Body::from(entry.response_body.clone()))
                        .unwrap()
                        .into_response();
                }
            }
        }
    }

    let is_cache_eligible = CacheKey::is_eligible(&payload);

    // Semantic cache (gated by config and no_store)
    let should_do_semantic = state.config.is_semantic_allowed() && !has_no_store;
    let target_coordinates: Option<Vec<f32>> = if should_do_semantic {
        match state.predictor.encode_text(&prompt) {
            Ok(v) => Some(v),
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Vector mapping failure").into_response(),
        }
    } else {
        None
    };

    // Scan vector space for high-affinity matches (gated by exact context)
    if should_do_semantic {
        if let Some(ref target_vec) = target_coordinates {
            let storage = state.index.read().unwrap();
            for item in storage.iter() {
                // Safety gate: only match within same context (system prompt, model, tenant)
                if item.context_key != context_key {
                    continue;
                }
                let score = compute_vector_dot(target_vec, &item.vector);
                if score >= ALIGNMENT_BAR {
                    println!("Semantic HIT! Similarity Score: {:.4}. Bypassing upstream latency entirely.", score);
                    if is_streaming {
                        let cached = item.completion_response.clone();
                        let stream = futures_util::stream::once(async move {
                            Ok::<_, std::io::Error>(Bytes::from(cached))
                        });
                        let body = Body::from_stream(stream);
                        return Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("x-stack-intercept", "hit")
                            .body(body)
                            .unwrap()
                            .into_response();
                    } else {
                        return Response::builder()
                            .header("content-type", "application/json")
                            .header("x-stack-intercept", "hit")
                            .body(Body::from(item.completion_response.clone()))
                            .unwrap()
                            .into_response();
                    }
                }
            }
        }
    }

    // Cache MISS -> Pipe out to upstream provider
    let upstream_url = format!("{}/v1/chat/completions", state.config.upstream_base_url);
    let upstream_res = state.client.post(&upstream_url)
        .header("authorization", orig_auth)
        .json(&payload)
        .send()
        .await;

    match upstream_res {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            if is_streaming {
                let prompt_clone = prompt.clone();
                let vector_clone = target_coordinates.clone();
                let state_clone = Arc::clone(&state);
                let cache_key_hash_clone = cache_key_hash.clone();
                let buffered_body = Arc::new(std::sync::Mutex::new(String::new()));
                let buffered_body_clone = Arc::clone(&buffered_body);

                let stream = res.bytes_stream().map(move |chunk_result| {
                    match chunk_result {
                        Ok(bytes) => {
                            let raw_str = String::from_utf8_lossy(&bytes).to_string();
                            {
                                let mut buf = buffered_body.lock().unwrap();
                                buf.push_str(&raw_str);
                            }
                            Ok(bytes)
                        },
                        Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
                    }
                });

                // When the stream ends, flush buffered content to cache
                let stream = stream.chain(stream::once({
                    let state_chain = Arc::clone(&state_clone);
                    let prompt_chain = prompt_clone.clone();
                    let vector_chain = vector_clone.clone();
                    let key_hash_chain = cache_key_hash_clone.clone();
                    async move {
                        let final_content = buffered_body_clone.lock().unwrap().clone();
                        if !final_content.is_empty() {
                            if is_cache_eligible {
                                if let Some(ref key_hash) = key_hash_chain {
                                    let mut cache = state_chain.exact_cache.write().unwrap();
                                    cache.insert(key_hash.clone(), final_content.clone());
                                    println!("Stream cached (exact).");
                                }
                            }
                            if should_do_semantic {
                                if let Some(ref vector) = vector_chain {
                                    let mut writer = state_chain.index.write().unwrap();
                                    writer.push(CacheItem {
                                        prompt: prompt_chain.clone(),
                                        vector: vector.clone(),
                                        completion_response: final_content,
                                        context_key: context_key.clone(),
                                    });
                                    println!("Stream cached via semantic coordinates.");
                                }
                            }
                        }
                        // Return an empty Ok that the client won't see as meaningful SSE
                        Ok::<_, std::io::Error>(Bytes::new())
                    }
                }));
                let body = Body::from_stream(stream);
                return Response::builder()
                    .header("content-type", "text/event-stream")
                    .header("cache-control", "no-store")
                    .header("x-stack-intercept", "miss")
                    .body(body)
                    .unwrap()
                    .into_response();
            }

            let bytes = res.bytes().await.unwrap_or_default();
            let res_str = String::from_utf8_lossy(&bytes).to_string();

            if is_cache_eligible {
                if let Some(ref key_hash) = cache_key_hash {
                    let mut cache = state.exact_cache.write().unwrap();
                    cache.insert(key_hash.clone(), res_str.clone());
                }
            }

            if should_do_semantic {
                if let Some(ref target_vec) = target_coordinates {
                    let mut writer = state.index.write().unwrap();
                    writer.push(CacheItem {
                        prompt: prompt.to_string(),
                        vector: target_vec.clone(),
                        completion_response: res_str.clone(),
                        context_key: context_key.clone(),
                    });
                }
            }

            (status, res_str).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Upstream Timeout").into_response(),
    }
}
