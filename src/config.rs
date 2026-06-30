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
    pub admin_key: Option<String>,
    pub exact_max_entries: usize,
    pub exact_ttl_secs: u64,
    pub semantic_max_items: usize,
    pub semantic_max_bucket_items: usize,
    pub semantic_ttl_secs: u64,
    pub cache_path: Option<String>,
    pub disable_persistence: bool,
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
            .unwrap_or(false);

        let upstream_base_url = env::var("STACK_INTERCEPT_UPSTREAM_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

        let fallback_base_url = env::var("STACK_INTERCEPT_FALLBACK_URL")
            .unwrap_or_else(|_| "https://api.deepseek.com".to_string());

        // Prefer the explicit fallback key; fall back to DEEPSEEK_API_KEY for convenience
        let fallback_api_key = env::var("STACK_INTERCEPT_FALLBACK_API_KEY")
            .ok()
            .or_else(|| env::var("DEEPSEEK_API_KEY").ok());

        let admin_key = env::var("STACK_INTERCEPT_ADMIN_KEY").ok();

        let exact_max_entries = env::var("STACK_INTERCEPT_EXACT_MAX_ENTRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20000);

        let exact_ttl_secs = env::var("STACK_INTERCEPT_EXACT_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let semantic_max_items = env::var("STACK_INTERCEPT_SEMANTIC_MAX_ITEMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10000);

        let semantic_max_bucket_items = env::var("STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256);

        let semantic_ttl_secs = env::var("STACK_INTERCEPT_SEMANTIC_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        let cache_path = env::var("STACK_INTERCEPT_CACHE_PATH").ok();

        let disable_persistence = env::var("STACK_INTERCEPT_DISABLE_PERSISTENCE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Self {
            cache_mode,
            tenant_id_header,
            allow_model_rewrite,
            max_body_size: 5 * 1024 * 1024, // 5 MB
            upstream_base_url,
            fallback_base_url,
            fallback_api_key,
            admin_key,
            exact_max_entries,
            exact_ttl_secs,
            semantic_max_items,
            semantic_max_bucket_items,
            semantic_ttl_secs,
            cache_path,
            disable_persistence,
        }
    }

    pub fn is_semantic_allowed(&self) -> bool {
        self.cache_mode == CacheMode::Semantic
    }

    pub fn is_cache_enabled(&self) -> bool {
        self.cache_mode != CacheMode::Off
    }
}
