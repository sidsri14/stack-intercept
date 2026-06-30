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
            match v.parse() {
                Ok(n) => self.exact_max_entries = n,
                Err(_) => eprintln!("WARNING: STACK_INTERCEPT_EXACT_MAX_ENTRIES='{}' is not a valid number, ignoring", v),
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_EXACT_TTL_SECS") {
            match v.parse() {
                Ok(n) => self.exact_ttl_secs = n,
                Err(_) => eprintln!("WARNING: STACK_INTERCEPT_EXACT_TTL_SECS='{}' is not a valid number, ignoring", v),
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_ITEMS") {
            match v.parse() {
                Ok(n) => self.semantic_max_items = n,
                Err(_) => eprintln!("WARNING: STACK_INTERCEPT_SEMANTIC_MAX_ITEMS='{}' is not a valid number, ignoring", v),
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS") {
            match v.parse() {
                Ok(n) => self.semantic_max_bucket_items = n,
                Err(_) => eprintln!("WARNING: STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS='{}' is not a valid number, ignoring", v),
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_TTL_SECS") {
            match v.parse() {
                Ok(n) => self.semantic_ttl_secs = n,
                Err(_) => eprintln!("WARNING: STACK_INTERCEPT_SEMANTIC_TTL_SECS='{}' is not a valid number, ignoring", v),
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
    use std::sync::Mutex;

    /// Serializes config tests that modify env vars (cargo test runs in parallel).
    static CONFIG_TEST_MUTEX: Mutex<()> = Mutex::new(());

    // Test 1a: Subprocess helper — only runs when invoked by
    // test_config_file_missing_exits via subprocess. Do not run standalone.
    // This function calls ProxyConfig::load() with STACK_INTERCEPT_CONFIG
    // pointing to a non-existent file, which triggers process::exit(1).
    // We detect this by checking if STACK_INTERCEPT_CONFIG is set (indicating
    // we're running as a subprocess rather than standalone).
    #[test]
    fn config_fatal_missing_file_subprocess_check() {
        // Only run when STACK_INTERCEPT_CONFIG signals a missing file.
        let config_path = match std::env::var("STACK_INTERCEPT_CONFIG") {
            Ok(p) if p.contains("does-not-exist") => p,
            _ => return, // Skip when run standalone
        };
        // If the file actually exists, skip (someone might have created it)
        if std::path::Path::new(&config_path).exists() {
            return;
        }
        // This will call process::exit(1) because the file doesn't exist.
        // If it returns, the test fails.
        ProxyConfig::load();
        panic!("ProxyConfig::load() should have called process::exit(1) for missing config file");
    }

    // Test 1b: Verify that explicit STACK_INTERCEPT_CONFIG + missing file → exit(1).
    #[test]
    fn test_config_file_missing_exits() {
        let _lock = CONFIG_TEST_MUTEX.lock().unwrap();

        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new(&exe)
            .env("STACK_INTERCEPT_CONFIG", "/tmp/__stack_intercept_does-not-exist-test.toml")
            .args(&["config_fatal_missing_file_subprocess_check", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "Expected exit code 1 (process::exit) for missing config file; got success"
        );
    }

    // Test 2: TOML file values are loaded via apply_file_config.
    #[test]
    fn test_config_file_values_loaded() {
        let _lock = CONFIG_TEST_MUTEX.lock().unwrap();

        let dir = std::env::temp_dir().join(format!("stack_intercept_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("test_config.toml");
        let toml_content = r#"
            upstream_url = "https://custom-upstream.example.com"
            exact_max_entries = 5000
            exact_ttl_secs = 7200
            cache_mode = "semantic"
            tenant_id_header = "X-Custom-Tenant"
            allow_model_rewrite = true
        "#;
        std::fs::write(&toml_path, toml_content).expect("failed to write test TOML");

        // Save old env var and set new one
        let old_val = std::env::var("STACK_INTERCEPT_CONFIG").ok();
        std::env::set_var(
            "STACK_INTERCEPT_CONFIG",
            toml_path.to_str().unwrap(),
        );

        let cfg = ProxyConfig::load();

        // Restore old env var
        match old_val {
            Some(v) => std::env::set_var("STACK_INTERCEPT_CONFIG", v),
            None => std::env::remove_var("STACK_INTERCEPT_CONFIG"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(cfg.upstream_base_url, "https://custom-upstream.example.com");
        assert_eq!(cfg.exact_max_entries, 5000);
        assert_eq!(cfg.exact_ttl_secs, 7200);
        assert_eq!(cfg.cache_mode, CacheMode::Semantic);
        assert_eq!(cfg.tenant_id_header, Some("X-Custom-Tenant".to_string()));
        assert!(cfg.allow_model_rewrite);
    }

    // Test 3: Env vars override TOML values.
    #[test]
    fn test_env_overrides_toml() {
        let _lock = CONFIG_TEST_MUTEX.lock().unwrap();

        let dir = std::env::temp_dir().join(format!("stack_intercept_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let toml_path = dir.join("test_override.toml");
        let toml_content = r#"
            exact_max_entries = 5000
            exact_ttl_secs = 3600
        "#;
        std::fs::write(&toml_path, toml_content).expect("failed to write test TOML");

        let old_cfg = std::env::var("STACK_INTERCEPT_CONFIG").ok();
        let old_max = std::env::var("STACK_INTERCEPT_EXACT_MAX_ENTRIES").ok();
        let old_ttl = std::env::var("STACK_INTERCEPT_EXACT_TTL_SECS").ok();

        std::env::set_var("STACK_INTERCEPT_CONFIG", toml_path.to_str().unwrap());
        std::env::set_var("STACK_INTERCEPT_EXACT_MAX_ENTRIES", "9999");
        std::env::remove_var("STACK_INTERCEPT_EXACT_TTL_SECS");

        let cfg = ProxyConfig::load();

        // Restore env vars
        match old_cfg {
            Some(v) => std::env::set_var("STACK_INTERCEPT_CONFIG", v),
            None => std::env::remove_var("STACK_INTERCEPT_CONFIG"),
        }
        match old_max {
            Some(v) => std::env::set_var("STACK_INTERCEPT_EXACT_MAX_ENTRIES", v),
            None => std::env::remove_var("STACK_INTERCEPT_EXACT_MAX_ENTRIES"),
        }
        match old_ttl {
            Some(v) => std::env::set_var("STACK_INTERCEPT_EXACT_TTL_SECS", v),
            None => std::env::remove_var("STACK_INTERCEPT_EXACT_TTL_SECS"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        // Env (9999) should override TOML (5000)
        assert_eq!(cfg.exact_max_entries, 9999);
        // TOML value should apply since no env var set
        assert_eq!(cfg.exact_ttl_secs, 3600);
    }

    // Test 4: Unknown TOML key fails (deny_unknown_fields).
    #[test]
    fn test_unknown_toml_key_fails() {
        let result: Result<FileConfig, _> = toml::from_str(r#"
            unknown_field = "this should fail"
        "#);
        assert!(result.is_err(), "deny_unknown_fields should reject unknown keys");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unknown_field") || err_msg.contains("unknown field"),
            "Error should mention the unknown field, got: {}",
            err_msg
        );
    }

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
