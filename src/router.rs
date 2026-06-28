use serde_json::Value;

/// Current routing policy version. Increment when routing logic changes
/// to invalidate old cache entries that may have been stored under a
/// different routing decision namespace.
const ROUTING_POLICY_VERSION: &str = "v1";

pub struct RouteDecision {
    pub final_url: String,
    pub final_model: String,
    pub needs_fallback_key: bool,
}

impl RouteDecision {
    /// Return a deterministic cache namespace string that uniquely identifies
    /// this routing decision. Both passthrough and fallback paths produce a
    /// namespace so that future routing policy changes don't accidentally
    /// share cache keys.
    pub fn cache_namespace(&self, upstream_url: &str, requested_model: &str) -> String {
        if self.needs_fallback_key {
            format!(
                "{}|fallback|{}|{}",
                ROUTING_POLICY_VERSION,
                self.final_url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches("/v1/chat/completions"),
                self.final_model,
            )
        } else {
            format!(
                "{}|passthrough|{}|{}",
                ROUTING_POLICY_VERSION,
                upstream_url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://"),
                requested_model,
            )
        }
    }
}

/// Collect all text content from all messages (system, developer, user),
/// lowercased, for high-reasoning and explicit-requirement keyword matching.
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

/// Check whether any message contains an explicit requirement to use a
/// specific model or provider — phrases like "use gpt-4o exactly",
/// "must use claude", "do not switch models", "require gpt-4", etc.
fn has_explicit_model_requirement(text: &str) -> bool {
    let strict_patterns = [
        "do not switch",
        "do not downgrade",
        "must use",
        "require gpt",
        "require claude",
        "use gpt-4o exactly",
        "use claude-3 exactly",
        "use claude opus exactly",
        "do not change the model",
        "keep this model",
        "use exactly",
        "do not route",
    ];
    strict_patterns.iter().any(|p| text.contains(p))
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

    // Check for explicit model/provider requirements — these always force passthrough
    if has_explicit_model_requirement(&all_text) {
        return decision;
    }

    // High-reasoning indicators — skip routing for these
    let needs_high_reasoning = all_text.contains("architect")
        // Math & proofs
        || all_text.contains("optimize")
        || all_text.contains("compile")
        || all_text.contains("cryptography")
        || all_text.contains("mathematical")
        || all_text.contains("calculus")
        || all_text.contains("linear algebra")
        || all_text.contains("statistical")
        || all_text.contains("theorem")
        || all_text.contains("proof")
        || all_text.contains("formal verification")
        // Debugging & systems
        || all_text.contains("race condition")
        || all_text.contains("deadlock")
        || all_text.contains("memory safety")
        || all_text.contains("distributed systems")
        || all_text.contains("consensus algorithm")
        || all_text.contains("concurrent")
        // Security
        || all_text.contains("security review")
        || all_text.contains("vulnerability assessment")
        || all_text.contains("penetration test")
        || all_text.contains("exploit")
        // Legal & analysis
        || all_text.contains("legal")
        || all_text.contains("contract analysis")
        || all_text.contains("compliance")
        // Financial
        || all_text.contains("financial model")
        || all_text.contains("monte carlo")
        || all_text.contains("risk analysis")
        // Database & query
        || all_text.contains("query planner")
        || all_text.contains("query optimization")
        || all_text.contains("transaction isolation");

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
