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

// --- SEMANTIC CACHE DATA STRUCTURES ---
#[derive(Serialize, Deserialize, Clone, Debug)]
struct CacheItem {
    prompt: String,
    vector: Vec<f32>,
    completion_response: String,
}

struct AppState {
    index: RwLock<Vec<CacheItem>>,
    client: Client,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let shared_state = Arc::new(AppState {
        index: RwLock::new(Vec::new()),
        client: Client::new(),
    });

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_intercept))
        .with_state(shared_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    println!("StackIntercept engine online | Production SSE engine on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handle_intercept(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let orig_auth = headers.get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // 1. Extract prompt string safely from OpenAI structure
    let prompt = payload["messages"]
        .as_array()
        .and_then(|msg| msg.last())
        .and_then(|last_msg| last_msg["content"].as_str())
        .unwrap_or("")
        .to_string();

    let is_streaming = payload["stream"].as_bool().unwrap_or(false);

    // TODO: Wire your Candle embedding generator here:
    // let current_vector = embedder.gen(prompt);

    // 2. Perform string-match scan against your state vector store
    {
        let cache = state.index.read().unwrap();
        for item in cache.iter() {
            if item.prompt == prompt {
                println!("Cache HIT for stackintercept: Return immediate zero-overhead response!");
                if is_streaming {
                    return handle_cached_stream(item.completion_response.clone()).into_response();
                } else {
                    return (StatusCode::OK, item.completion_response.clone()).into_response();
                }
            }
        }
    }

    // 3. Cache MISS -> Forward request to upstream provider
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
                let state_clone = Arc::clone(&state);
                let buffered_stream_content = Arc::new(std::sync::Mutex::new(String::new()));

                let stream = res.bytes_stream().map({
                    let buf = Arc::clone(&buffered_stream_content);
                    let prompt = prompt_clone.clone();
                    let st = Arc::clone(&state_clone);
                    move |chunk_result| {
                        match chunk_result {
                            Ok(bytes) => {
                                let raw_str = String::from_utf8_lossy(&bytes).to_string();
                                // Concurrently buffer raw SSE chunks into string memory space
                                if let Ok(mut buf_guard) = buf.lock() {
                                    buf_guard.push_str(&raw_str);
                                }

                                // Look for end of stream indicator to safely commit response to cache
                                if raw_str.contains("[DONE]") {
                                    let final_content = {
                                        let guard = buf.lock().unwrap_or_else(|e| e.into_inner());
                                        guard.clone()
                                    };
                                    let mut index_write = st.index.write().unwrap();
                                    index_write.push(CacheItem {
                                        prompt: prompt.clone(),
                                        vector: vec![0.0], // Fill with real Candle vector outputs in step 3
                                        completion_response: final_content,
                                    });
                                    println!("Stream complete. Saved to StackIntercept cache.");
                                }
                                Ok::<Event, Infallible>(Event::default().data(raw_str))
                            },
                            Err(_) => Ok(Event::default().data("[ERROR]")),
                        }
                    }
                });
                return Sse::new(stream).into_response();
            }

            // Non-stream handler
            let bytes = res.bytes().await.unwrap_or_default();
            let res_str = String::from_utf8_lossy(&bytes).to_string();

            // Commit flat non-streaming response directly to cache
            let mut index_write = state.index.write().unwrap();
            index_write.push(CacheItem {
                prompt: prompt.clone(),
                vector: vec![0.0],
                completion_response: res_str.clone(),
            });

            (status, res_str).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "StackIntercept gateway drop").into_response(),
    }
}

// Emulate an ultra-fast local OpenAI Server-Sent Event stream using tokens out of memory storage
fn handle_cached_stream(cached_raw_sse: String) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let lines: Vec<String> = cached_raw_sse.lines().map(|s| s.to_string()).collect();
    let stream = futures_util::stream::iter(lines).map(|line| {
        Ok(Event::default().data(line))
    });
    Sse::new(stream)
}
