use serde_json::Value;

pub struct RouteDecision {
    pub final_url: String,
    pub final_model: String,
    pub needs_fallback_key: bool,
}

/// Collect all text content from all messages (system, developer, user),
/// lowercased, for high-reasoning keyword matching.
fn collect_all_text(payload: &Value) -> String {
    let mut texts = Vec::new();
    if let Some(messages) = payload["messages"].as_array() {
        for msg in messages {
            if let Some(content) = msg["content"].as_str() {
                texts.push(content.to_lowercase());
            }
        }
    }
    texts.join(" ")
}

/// Check if any message contains non-text (multimodal) content.
fn has_multimodal_content(payload: &Value) -> bool {
    if let Some(messages) = payload["messages"].as_array() {
        for msg in messages {
            if msg["content"].is_array() {
                return true;
            }
        }
    }
    false
}

/// Check whether the payload contains features unsafe for routing.
/// Routing is blocked when the request uses tools, structured output,
/// non-deterministic temperature, or multimodal inputs.
fn has_unsafe_features(payload: &Value) -> bool {
    // Tools
    if let Some(tools) = payload["tools"].as_array() {
        if !tools.is_empty() {
            return true;
        }
    }
    // Response format (structured output)
    if payload["response_format"].is_object() {
        return true;
    }
    // Tool choice
    if !payload["tool_choice"].is_null() {
        return true;
    }
    // Non-deterministic temperature
    if let Some(temp) = payload["temperature"].as_f64() {
        if temp > 0.0 {
            return true;
        }
    }
    // Multimodal content (images, files, etc.)
    if has_multimodal_content(payload) {
        return true;
    }
    false
}

/// Inspects the inbound payload and decides whether to offload the request
/// to a cheaper upstream model (e.g., deepseek-chat) when the user requested
/// a premium model for a simple task.
pub fn evaluate_routing(
    payload: &Value,
    upstream_url: &str,
    fallback_url: &str,
    allow_rewrite: bool,
    no_route: bool,
) -> RouteDecision {
    let requested_model = payload["model"].as_str().unwrap_or("unknown");

    // Default: passthrough with no changes
    let mut decision = RouteDecision {
        final_url: format!("{}/v1/chat/completions", upstream_url),
        final_model: requested_model.to_string(),
        needs_fallback_key: false,
    };

    // Routing must be explicitly enabled and not blocked by header
    if !allow_rewrite || no_route {
        return decision;
    }

    // Block routing for requests with unsafe features
    if has_unsafe_features(payload) {
        return decision;
    }

    // Collect all message text (lowercased) for analysis
    let all_text = collect_all_text(payload);

    // High-reasoning indicators — skip routing for these
    let needs_high_reasoning = all_text.contains("architect")
        || all_text.contains("optimize")
        || all_text.contains("compile")
        || all_text.contains("cryptography")
        || all_text.contains("mathematical");

    // Premium models eligible for downgrade
    let is_premium = requested_model.contains("gpt-4")
        || requested_model.contains("claude-3-5")
        || requested_model.contains("claude-3-opus");

    if is_premium && !needs_high_reasoning {
        println!(
            "Route triggered: Downgrading {} to deepseek-chat",
            requested_model
        );
        decision.final_url = format!("{}/v1/chat/completions", fallback_url);
        decision.final_model = "deepseek-chat".to_string();
        decision.needs_fallback_key = true;
    }

    decision
}
