use std::env;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheMode {
    Off,
    Exact,
    Semantic,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub cache_mode: CacheMode,
    pub tenant_id_header: Option<String>,
    pub allow_model_rewrite: bool,
    pub max_body_size: usize,
    pub upstream_base_url: String,
    pub fallback_base_url: String,
    pub fallback_api_key: Option<String>,
}

impl ProxyConfig {
    pub fn from_env() -> Self {
        let cache_mode = match env::var("STACK_INTERCEPT_CACHE_MODE")
            .unwrap_or_else(|_| "exact".to_string())
            .as_str()
        {
            "off" => CacheMode::Off,
            "semantic" => CacheMode::Semantic,
            _ => CacheMode::Exact,
        };

        let tenant_id_header = env::var("STACK_INTERCEPT_TENANT_ID_HEADER").ok();

        let allow_model_rewrite = env::var("STACK_INTERCEPT_ALLOW_MODEL_REWRITE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        let upstream_base_url = env::var("STACK_INTERCEPT_UPSTREAM_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

        let fallback_base_url = env::var("STACK_INTERCEPT_FALLBACK_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

        // Prefer the explicit fallback key; fall back to DEEPSEEK_API_KEY for convenience
        let fallback_api_key = env::var("STACK_INTERCEPT_FALLBACK_API_KEY")
            .ok()
            .or_else(|| env::var("DEEPSEEK_API_KEY").ok());

        Self {
            cache_mode,
            tenant_id_header,
            allow_model_rewrite,
            max_body_size: 5 * 1024 * 1024, // 5 MB
            upstream_base_url,
            fallback_base_url,
            fallback_api_key,
        }
    }

    pub fn is_semantic_allowed(&self) -> bool {
        self.cache_mode == CacheMode::Semantic
    }

    pub fn is_cache_enabled(&self) -> bool {
        self.cache_mode != CacheMode::Off
    }
}
