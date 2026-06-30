#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheMode {
    Off,
    Exact,
    Semantic,
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    cache_mode: Option<String>,
    tenant_id_header: Option<String>,
    allow_model_rewrite: Option<bool>,
    upstream_url: Option<String>,
    fallback_url: Option<String>,
    fallback_api_key: Option<String>,
    admin_key: Option<String>,
    exact_max_entries: Option<usize>,
    exact_ttl_secs: Option<u64>,
    semantic_max_items: Option<usize>,
    semantic_max_bucket_items: Option<usize>,
    semantic_ttl_secs: Option<u64>,
    cache_path: Option<String>,
    disable_persistence: Option<bool>,
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
    pub fn defaults() -> Self {
        Self {
            cache_mode: CacheMode::Exact,
            tenant_id_header: None,
            allow_model_rewrite: false,
            max_body_size: 5 * 1024 * 1024,
            upstream_base_url: "https://api.deepseek.com".to_string(),
            fallback_base_url: "https://api.deepseek.com".to_string(),
            fallback_api_key: None,
            admin_key: None,
            exact_max_entries: 20000,
            exact_ttl_secs: 3600,
            semantic_max_items: 10000,
            semantic_max_bucket_items: 256,
            semantic_ttl_secs: 3600,
            cache_path: None,
            disable_persistence: false,
        }
    }

    pub fn from_env() -> Self {
        let mut config = Self::defaults();
        config.apply_env_overrides();
        config
    }

    pub fn load() -> Self {
        let mut config = Self::defaults();
        config.apply_file_config();
        config.apply_env_overrides();
        config
    }

    fn apply_file_config(&mut self) {
        let config_path = std::env::var("STACK_INTERCEPT_CONFIG").ok();
        let toml_str = match &config_path {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "FATAL: STACK_INTERCEPT_CONFIG={} — file not found: {}",
                        path, e
                    );
                    std::process::exit(1);
                }
            },
            None => {
                let default_path = std::path::Path::new("stack-intercept.toml");
                if default_path.exists() {
                    match std::fs::read_to_string(default_path) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!(
                                "FATAL: stack-intercept.toml exists but cannot be read: {}",
                                e
                            );
                            std::process::exit(1);
                        }
                    }
                } else {
                    return;
                }
            }
        };
        let file_config: FileConfig = match toml::from_str(&toml_str) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("FATAL: config file parse error: {}", e);
                std::process::exit(1);
            }
        };
        if let Some(v) = file_config.cache_mode {
            self.cache_mode = match v.as_str() {
                "off" => CacheMode::Off,
                "semantic" => CacheMode::Semantic,
                _ => CacheMode::Exact,
            };
        }
        if let Some(v) = file_config.tenant_id_header {
            self.tenant_id_header = Some(v);
        }
        if let Some(v) = file_config.allow_model_rewrite {
            self.allow_model_rewrite = v;
        }
        if let Some(v) = file_config.upstream_url {
            self.upstream_base_url = v;
        }
        if let Some(v) = file_config.fallback_url {
            self.fallback_base_url = v;
        }
        if let Some(v) = file_config.fallback_api_key {
            self.fallback_api_key = Some(v);
        }
        if let Some(v) = file_config.admin_key {
            self.admin_key = Some(v);
        }
        if let Some(v) = file_config.exact_max_entries {
            self.exact_max_entries = v;
        }
        if let Some(v) = file_config.exact_ttl_secs {
            self.exact_ttl_secs = v;
        }
        if let Some(v) = file_config.semantic_max_items {
            self.semantic_max_items = v;
        }
        if let Some(v) = file_config.semantic_max_bucket_items {
            self.semantic_max_bucket_items = v;
        }
        if let Some(v) = file_config.semantic_ttl_secs {
            self.semantic_ttl_secs = v;
        }
        if let Some(v) = file_config.cache_path {
            self.cache_path = Some(v);
        }
        if let Some(v) = file_config.disable_persistence {
            self.disable_persistence = v;
        }
    }

    pub fn apply_env_overrides(&mut self) -> &mut Self {
        if let Ok(v) = std::env::var("STACK_INTERCEPT_CACHE_MODE") {
            self.cache_mode = match v.as_str() {
                "off" => CacheMode::Off,
                "semantic" => CacheMode::Semantic,
                _ => CacheMode::Exact,
            };
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_TENANT_ID_HEADER") {
            self.tenant_id_header = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_ALLOW_MODEL_REWRITE") {
            self.allow_model_rewrite = v == "true" || v == "1";
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_UPSTREAM_URL") {
            self.upstream_base_url = v;
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_FALLBACK_URL") {
            self.fallback_base_url = v;
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_FALLBACK_API_KEY") {
            self.fallback_api_key = Some(v);
        }
        if self.fallback_api_key.is_none() {
            if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
                self.fallback_api_key = Some(v);
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_ADMIN_KEY") {
            self.admin_key = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_EXACT_MAX_ENTRIES") {
            if let Ok(n) = v.parse() {
                self.exact_max_entries = n;
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_EXACT_TTL_SECS") {
            if let Ok(n) = v.parse() {
                self.exact_ttl_secs = n;
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_ITEMS") {
            if let Ok(n) = v.parse() {
                self.semantic_max_items = n;
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS") {
            if let Ok(n) = v.parse() {
                self.semantic_max_bucket_items = n;
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_TTL_SECS") {
            if let Ok(n) = v.parse() {
                self.semantic_ttl_secs = n;
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_CACHE_PATH") {
            self.cache_path = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_DISABLE_PERSISTENCE") {
            self.disable_persistence = v == "true" || v == "1";
        }
        self
    }

    pub fn is_semantic_allowed(&self) -> bool {
        self.cache_mode == CacheMode::Semantic
    }

    pub fn is_cache_enabled(&self) -> bool {
        self.cache_mode != CacheMode::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_cache_mode_exact() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.cache_mode, CacheMode::Exact);
    }

    #[test]
    fn test_defaults_exact_cache_sizes() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.exact_max_entries, 20000);
        assert_eq!(cfg.exact_ttl_secs, 3600);
    }

    #[test]
    fn test_defaults_semantic_sizes() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.semantic_max_items, 10000);
        assert_eq!(cfg.semantic_max_bucket_items, 256);
        assert_eq!(cfg.semantic_ttl_secs, 3600);
    }

    #[test]
    fn test_defaults_secrets_none() {
        let cfg = ProxyConfig::defaults();
        assert!(cfg.admin_key.is_none());
        assert!(cfg.fallback_api_key.is_none());
    }

    #[test]
    fn test_file_config_merge_none_is_noop() {
        let file_cfg = FileConfig::default();
        let mut cfg = ProxyConfig::defaults();
        let original = cfg.exact_max_entries;
        if let Some(v) = file_cfg.exact_max_entries { cfg.exact_max_entries = v; }
        assert_eq!(cfg.exact_max_entries, original);
    }

    #[test]
    fn test_file_config_merge_some_applies() {
        let file_cfg = FileConfig {
            exact_max_entries: Some(5000),
            exact_ttl_secs: Some(7200),
            ..Default::default()
        };
        let mut cfg = ProxyConfig::defaults();
        if let Some(v) = file_cfg.exact_max_entries { cfg.exact_max_entries = v; }
        if let Some(v) = file_cfg.exact_ttl_secs { cfg.exact_ttl_secs = v; }
        assert_eq!(cfg.exact_max_entries, 5000);
        assert_eq!(cfg.exact_ttl_secs, 7200);
        assert_eq!(cfg.semantic_max_items, 10000); // unchanged
    }

    #[test]
    fn test_cache_mode_from_str() {
        let mut cfg = ProxyConfig::defaults();

        // "off" -> Off
        cfg.cache_mode = match "off" {
            "off" => CacheMode::Off,
            "semantic" => CacheMode::Semantic,
            _ => CacheMode::Exact,
        };
        assert_eq!(cfg.cache_mode, CacheMode::Off);

        // "semantic" -> Semantic
        cfg.cache_mode = match "semantic" {
            "off" => CacheMode::Off,
            "semantic" => CacheMode::Semantic,
            _ => CacheMode::Exact,
        };
        assert_eq!(cfg.cache_mode, CacheMode::Semantic);

        // unknown -> Exact (default)
        cfg.cache_mode = match "unknown" {
            "off" => CacheMode::Off,
            "semantic" => CacheMode::Semantic,
            _ => CacheMode::Exact,
        };
        assert_eq!(cfg.cache_mode, CacheMode::Exact);
    }

    #[test]
    fn test_is_cache_enabled() {
        let mut cfg = ProxyConfig::defaults();
        cfg.cache_mode = CacheMode::Off;
        assert!(!cfg.is_cache_enabled());
        cfg.cache_mode = CacheMode::Exact;
        assert!(cfg.is_cache_enabled());
        cfg.cache_mode = CacheMode::Semantic;
        assert!(cfg.is_cache_enabled());
    }

    #[test]
    fn test_is_semantic_allowed() {
        let mut cfg = ProxyConfig::defaults();
        cfg.cache_mode = CacheMode::Off;
        assert!(!cfg.is_semantic_allowed());
        cfg.cache_mode = CacheMode::Exact;
        assert!(!cfg.is_semantic_allowed());
        cfg.cache_mode = CacheMode::Semantic;
        assert!(cfg.is_semantic_allowed());
    }
}
