use config::Environment;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

static INSTANCE: OnceLock<Arc<Config>> = OnceLock::new();

#[derive(Clone, Deserialize)]
pub struct Config {
    pub database_url: String,
    pub salt: String,
    #[serde(default = "defaults::server_addr")]
    pub server_addr: String,
    #[serde(default = "defaults::thumbnail_storage")]
    pub thumbnail_storage: String,
    #[serde(default = "defaults::base_url")]
    pub base_url: String,
    #[serde(default = "defaults::token_expiration_days")]
    pub token_expiration_days: i64,
    #[serde(default = "defaults::token_length")]
    pub token_length: usize,
    #[serde(default = "defaults::plugin_dir")]
    pub plugin_dir: String,
    #[serde(default = "defaults::ignore_patterns")]
    pub ignore_patterns: Vec<String>,
    /// Comma-separated list of origins allowed to make cross-origin requests
    /// (CORS). Empty by default — same-origin requests are never subject to
    /// CORS, so this only needs to be set when the frontend is served from a
    /// different origin than the API.
    #[serde(default)]
    pub cors_allowed_origins: String,
    /// Whether to trust `X-Forwarded-For` / `X-Real-IP` headers for the
    /// client IP recorded on session tokens. Only enable this when the
    /// server sits behind a reverse proxy that sets these headers itself —
    /// otherwise any client can spoof them. Defaults to `false`, falling
    /// back to the real TCP peer address.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
    /// Minimum cosine similarity for a face to be matched to a known contact.
    /// The single "is this a known person" threshold shared by the job pipeline
    /// and the CLI `face_match` tool (see `crate::face_match`).
    #[serde(default = "defaults::face_match_threshold")]
    pub face_match_threshold: f32,
    /// Minimum gap between the best contact's similarity and the best
    /// *different* contact's similarity. Rejects ambiguous matches where two
    /// people are almost equally close. Set to `0.0` to disable the guard.
    #[serde(default = "defaults::face_match_margin")]
    pub face_match_margin: f32,
    /// Free-form key/value configuration handed to every classifier plugin's
    /// `init`. Populated from `BYTEBURROW__PLUGIN__<KEY>` environment variables
    /// (e.g. `BYTEBURROW__PLUGIN__OLLAMA_URL` → key `ollama_url`); each plugin
    /// reads the keys it recognizes and ignores the rest. The keys every
    /// bundled plugin accepts are documented in `docs/architecture.md` and
    /// `.env.example`.
    #[serde(default)]
    pub plugin: HashMap<String, String>,
}

mod defaults {
    pub fn server_addr() -> String {
        "0.0.0.0:3000".to_string()
    }
    pub fn thumbnail_storage() -> String {
        "/tmp/thumbnails".to_string()
    }
    pub fn base_url() -> String {
        "http://localhost:3000".to_string()
    }
    pub fn token_expiration_days() -> i64 {
        30
    }
    pub fn token_length() -> usize {
        32
    }
    pub fn plugin_dir() -> String {
        "/etc/byteburrow/plugins".to_string()
    }
    pub fn face_match_threshold() -> f32 {
        0.8
    }
    pub fn face_match_margin() -> f32 {
        0.05
    }
    pub fn ignore_patterns() -> Vec<String> {
        vec![
            ".git".to_string(),
            ".cache".to_string(),
            "node_modules".to_string(),
            ".DS_Store".to_string(),
            "__pycache__".to_string(),
            ".Trash".to_string(),
        ]
    }
}

impl Config {
    pub fn get() -> Arc<Config> {
        INSTANCE.get().expect("Config not initialized").clone()
    }

    pub fn set(config: Arc<Config>) {
        INSTANCE
            .set(config)
            .unwrap_or_else(|_| panic!("Config already initialized"));
    }

    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        Self::from_source(
            Environment::with_prefix("BYTEBURROW")
                .separator("__")
                .try_parsing(true),
        )
    }

    /// Build the config from a prepared `Environment` source. Split out from
    /// `from_env` so tests can inject variables instead of mutating the
    /// process-wide environment.
    fn from_source(env: Environment) -> Self {
        config::Config::builder()
            .add_source(env)
            .build()
            .expect("Failed to build configuration")
            .try_deserialize()
            .expect("Failed to deserialize configuration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_from(vars: &[(&str, &str)]) -> Environment {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Environment::with_prefix("BYTEBURROW")
            .separator("__")
            .try_parsing(true)
            .source(Some(map))
    }

    #[test]
    fn plugin_section_collects_prefixed_vars() {
        // BYTEBURROW__PLUGIN__<KEY> lands in the `plugin` map as `<key>`, which
        // is exactly what the host forwards into every plugin's `init`.
        let config = Config::from_source(env_from(&[
            ("BYTEBURROW__DATABASE_URL", "postgres://x"),
            ("BYTEBURROW__SALT", "s"),
            ("BYTEBURROW__PLUGIN__OLLAMA_URL", "http://ollama:11434"),
            ("BYTEBURROW__PLUGIN__OLLAMA_MODEL", "llava"),
            ("BYTEBURROW__PLUGIN__FACE_MAX_DIM", "1024"),
        ]));

        assert_eq!(
            config.plugin.get("ollama_url").map(String::as_str),
            Some("http://ollama:11434")
        );
        assert_eq!(
            config.plugin.get("ollama_model").map(String::as_str),
            Some("llava")
        );
        assert_eq!(
            config.plugin.get("face_max_dim").map(String::as_str),
            Some("1024")
        );
    }

    #[test]
    fn plugin_section_defaults_empty_and_does_not_swallow_plugin_dir() {
        // `PLUGIN_DIR` (single underscore) is a top-level key, not part of the
        // `PLUGIN__` (double underscore) plugin section.
        let config = Config::from_source(env_from(&[
            ("BYTEBURROW__DATABASE_URL", "postgres://x"),
            ("BYTEBURROW__SALT", "s"),
            ("BYTEBURROW__PLUGIN_DIR", "/opt/plugins"),
        ]));

        assert!(config.plugin.is_empty());
        assert_eq!(config.plugin_dir, "/opt/plugins");
    }
}
