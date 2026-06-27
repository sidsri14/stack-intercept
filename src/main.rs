pub mod embeddings;

use axum::{
    routing::post,
    Router,
    response::{IntoResponse, Sse, sse::Event},
    http::{StatusCode, HeaderMap},
    Json,
    extract::State,
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::convert::Infallible;
use crate::embeddings::SemanticEmbedder;

const SIMILARITY_THRESHOLD: f32 = 0.92;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct CacheItem {
    prompt: String,
    vector: Vec<f32>,
    completion_response: String,
}

struct AppState {
    embedder: SemanticEmbedder,
    cache_store: RwLock<Vec<CacheItem>>,
    client: Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    println!("Initializing BGE Local Embedding Weights...");
    let embedder = SemanticEmbedder::load().expect("Failed to bind local model weights");

    let shared_state = Arc::new(AppState {
        embedder,
        cache_store: RwLock::new(Vec::new()),
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

fn cosine_similarity(v1: &[f32], v2: &[f32]) -> f32 {
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

    // Generate real-time semantic vector mapping
    let current_vector = match state.embedder.generate_vector(&prompt) {
        Ok(v) => v,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "Vector mapping failure").into_response(),
    };

    // Scan vector space for high-affinity matches
    {
        let storage = state.cache_store.read().unwrap();
        for item in storage.iter() {
            let score = cosine_similarity(&current_vector, &item.vector);
            if score >= SIMILARITY_THRESHOLD {
                println!("Semantic HIT! Similarity Score: {:.4}. Bypassing upstream latency entirely.", score);
                if is_streaming {
                    return handle_cached_stream(item.completion_response.clone()).into_response();
                } else {
                    return (StatusCode::OK, item.completion_response.clone()).into_response();
                }
            }
        }
    }

    // Cache MISS -> Pipe out to OpenAI
    let upstream_res = state.client.post("https://api.openai.com/v1/chat/completions")
        .header("authorization", orig_auth)
        .json(&payload)
        .send()
        .await;

    match upstream_res {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            if is_streaming {
                let prompt_clone = prompt.clone();
                let vector_clone = current_vector.clone();
                let state_clone = Arc::clone(&state);
                let mut buffered_stream_content = String::new();

                let stream = res.bytes_stream().map(move |chunk_result| {
                    match chunk_result {
                        Ok(bytes) => {
                            let raw_str = String::from_utf8_lossy(&bytes).to_string();
                            buffered_stream_content.push_str(&raw_str);

                            if raw_str.contains("[DONE]") {
                                let mut writer = state_clone.cache_store.write().unwrap();
                                writer.push(CacheItem {
                                    prompt: prompt_clone.clone(),
                                    vector: vector_clone.clone(),
                                    completion_response: buffered_stream_content.clone(),
                                });
                                println!("Stream cached via semantic coordinates.");
                            }
                            Ok::<Event, Infallible>(Event::default().data(raw_str))
                        },
                        Err(_) => Ok(Event::default().data("[ERROR]")),
                    }
                });
                return Sse::new(stream).into_response();
            }

            let bytes = res.bytes().await.unwrap_or_default();
            let res_str = String::from_utf8_lossy(&bytes).to_string();

            let mut writer = state.cache_store.write().unwrap();
            writer.push(CacheItem {
                prompt: prompt.to_string(),
                vector: current_vector,
                completion_response: res_str.clone(),
            });

            (status, res_str).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Upstream Timeout").into_response(),
    }
}

fn handle_cached_stream(cached_raw_sse: String) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let lines: Vec<String> = cached_raw_sse.lines().map(|s| s.to_string()).collect();
    let stream = futures_util::stream::iter(lines).map(|line| Ok(Event::default().data(line)));
    Sse::new(stream)
}
