//! Environment-based configuration.

use std::env;

/// Runtime configuration, read from environment variables with defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub clickhouse_url: String,
    pub clickhouse_user: String,
    pub clickhouse_password: String,
    pub clickhouse_database: String,
    pub listen_addr: String,
}

impl Config {
    /// Reads the configuration from process environment variables.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| env::var(key).ok())
    }

    /// Reads the configuration through an arbitrary lookup function
    /// (injectable for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let var = |key: &str, default: &str| get(key).unwrap_or_else(|| default.to_string());
        Self {
            clickhouse_url: var("CLICKHOUSE_URL", "http://localhost:8123"),
            clickhouse_user: var("CLICKHOUSE_USER", "dashboard"),
            clickhouse_password: var("CLICKHOUSE_PASSWORD", ""),
            clickhouse_database: var("CLICKHOUSE_DATABASE", "default"),
            listen_addr: var("LISTEN_ADDR", "0.0.0.0:8080"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_unset() {
        let config = Config::from_lookup(|_| None);
        assert_eq!(
            config,
            Config {
                clickhouse_url: "http://localhost:8123".to_string(),
                clickhouse_user: "dashboard".to_string(),
                clickhouse_password: String::new(),
                clickhouse_database: "default".to_string(),
                listen_addr: "0.0.0.0:8080".to_string(),
            }
        );
    }

    #[test]
    fn env_overrides_defaults() {
        let config = Config::from_lookup(|key| match key {
            "CLICKHOUSE_URL" => Some("http://ch:8123".to_string()),
            "LISTEN_ADDR" => Some("127.0.0.1:9999".to_string()),
            _ => None,
        });
        assert_eq!(config.clickhouse_url, "http://ch:8123");
        assert_eq!(config.listen_addr, "127.0.0.1:9999");
        assert_eq!(config.clickhouse_user, "dashboard");
    }
}
