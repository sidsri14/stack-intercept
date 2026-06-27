use axum::{
    routing::post,
    Router,
    response::{IntoResponse, Sse, sse::Event},
    http::StatusCode,
    Json,
    http::HeaderMap,
};
use futures_util::StreamExt;
use reqwest::Client;
use std::net::SocketAddr;
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/v1/chat/completions", post(handle_intercept));

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    println!("StackIntercept core engine operating at http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn handle_intercept(
    headers: HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let client = Client::new();

    let orig_auth = headers.get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // TODO: Pull down the user prompt from payload["messages"]
    // TODO: Perform local Candle semantic embedding lookup here.
    // If semantic cache hits -> return cached response instantly.

    // If cache misses, forward payload to upstream API
    let upstream_response = client.post("https://api.openai.com/v1/chat/completions")
        .header("authorization", orig_auth)
        .json(&payload)
        .send()
        .await;

    match upstream_response {
        Ok(res) => {
            let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

            // Handle streaming (SSE) passthrough
            if payload["stream"].as_bool().unwrap_or(false) {
                let stream = res.bytes_stream().map(|result| {
                    match result {
                        Ok(bytes) => {
                            // TODO: Concurrently append chunks to in-memory buffer
                            // to commit to semantic cache when stream closes.
                            Ok::<Event, reqwest::Error>(Event::default().data(String::from_utf8_lossy(&bytes).to_string()))
                        },
                        Err(e) => Err(e),
                    }
                });
                return Sse::new(stream).into_response();
            }

            // Fallback for non-streamed requests
            let bytes = res.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "StackIntercept Routing Failure").into_response(),
    }
}
