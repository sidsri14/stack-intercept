use serde_json::Value;

pub struct RouteDecision {
    pub final_url: String,
    pub final_model: String,
    pub needs_fallback_key: bool,
}

/// Inspects the inbound payload and decides whether to offload the request
/// to a cheaper upstream model (e.g., deepseek-chat) when the user requested
/// a premium model for a simple task.
pub fn evaluate_routing(
    payload: &Value,
    upstream_url: &str,
    fallback_url: &str,
    allow_rewrite: bool,
) -> RouteDecision {
    let requested_model = payload["model"].as_str().unwrap_or("unknown");

    // Default: passthrough with no changes
    let mut decision = RouteDecision {
        final_url: format!("{}/v1/chat/completions", upstream_url),
        final_model: requested_model.to_string(),
        needs_fallback_key: false,
    };

    if !allow_rewrite {
        return decision;
    }

    let prompt_text = payload["messages"]
        .as_array()
        .and_then(|msg| msg.last())
        .and_then(|last_msg| last_msg["content"].as_str())
        .unwrap_or("");

    // High-reasoning indicators — skip routing for these
    let needs_high_reasoning = prompt_text.contains("architect")
        || prompt_text.contains("optimize")
        || prompt_text.contains("compile")
        || prompt_text.contains("cryptography")
        || prompt_text.contains("mathematical");

    // Premium models eligible for downgrade
    let is_premium = requested_model.contains("gpt-4")
        || requested_model.contains("claude-3-5")
        || requested_model.contains("claude-3-opus");

    if is_premium && !needs_high_reasoning {
        println!(
            "Budget Route triggered: Downgrading {} to deepseek-chat",
            requested_model
        );
        decision.final_url = format!("{}/v1/chat/completions", fallback_url);
        decision.final_model = "deepseek-chat".to_string();
        decision.needs_fallback_key = true;
    }

    decision
}
