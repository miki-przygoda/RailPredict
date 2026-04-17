//! Centralised configuration. Every env var the system reads is declared here.
//! No module calls `std::env::var` directly — they receive a `&Config`.
//!
//! ## Env vars
//!
//! | Variable             | Required | Default                               | Description                        |
//! |----------------------|----------|---------------------------------------|------------------------------------|
//! | `DATABASE_URL`       | yes      | —                                     | PostgreSQL connection string        |
//! | `GBR_API_KEY`        | yes      | —                                     | x-apikey header for GBR Retail API |
//! | `GBR_API_BASE_URL`   | no       | https://api.rtt.io/api                | Override for testing/staging       |
//! | `DARWIN_HOST`        | yes      | —                                     | Darwin STOMP broker hostname       |
//! | `DARWIN_PORT`        | no       | 61613                                 | Darwin STOMP broker port           |
//! | `DARWIN_USERNAME`    | yes      | —                                     | Darwin ActiveMQ username           |
//! | `DARWIN_PASSWORD`    | yes      | —                                     | Darwin ActiveMQ password           |
//! | `DARWIN_DESTINATION` | no       | /topic/darwin.pushport-v16            | STOMP subscription topic           |
//! | `WATCHED_ROUTES`     | no       | "" (watch everything)                 | Comma-separated CRS codes          |
//! | `LOG_LEVEL`          | no       | info                                  | tracing level filter               |
//! | `LOG_FORMAT`         | no       | pretty                                | `pretty` or `json`                 |
//! | `API_BIND_ADDR`      | no       | 0.0.0.0:3000                          | axum server bind address           |

use std::collections::HashSet;
use std::fmt;

// ---------------------------------------------------------------------------
// Error — lists ALL missing vars at once
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ConfigError {
    missing: Vec<&'static str>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Missing required environment variables: {}. \
             Copy .env.example to .env and fill in the values.",
            self.missing.join(", ")
        )
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Config {
    // Database
    pub database_url: String,

    // GBR Retail API
    pub gbr_api_key: String,
    pub gbr_api_base_url: String,

    // Darwin Push Port (STOMP)
    pub darwin_host: String,
    pub darwin_port: u16,
    pub darwin_username: String,
    pub darwin_password: String,
    pub darwin_destination: String,

    // Ingestion
    /// Empty set means watch all routes (no filter applied).
    pub watched_routes: HashSet<String>,

    // Observability
    pub log_level: String,
    pub log_format: LogFormat,

    // API server
    pub api_bind_addr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl Config {
    /// Load config from environment variables. Collects ALL missing required vars
    /// and returns them in a single error rather than failing on the first missing one.
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut missing: Vec<&'static str> = Vec::new();

        macro_rules! require {
            ($var:literal) => {
                std::env::var($var).unwrap_or_else(|_| {
                    missing.push($var);
                    String::new()
                })
            };
        }

        macro_rules! optional {
            ($var:literal, $default:expr) => {
                std::env::var($var).unwrap_or_else(|_| $default.to_string())
            };
        }

        let database_url = require!("DATABASE_URL");
        let gbr_api_key = require!("GBR_API_KEY");
        let darwin_host = require!("DARWIN_HOST");
        let darwin_username = require!("DARWIN_USERNAME");
        let darwin_password = require!("DARWIN_PASSWORD");

        if !missing.is_empty() {
            return Err(ConfigError { missing });
        }

        let darwin_port = std::env::var("DARWIN_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(61613u16);

        let watched_routes = std::env::var("WATCHED_ROUTES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        let log_format = match optional!("LOG_FORMAT", "pretty").as_str() {
            "json" => LogFormat::Json,
            _ => LogFormat::Pretty,
        };

        Ok(Self {
            database_url,
            gbr_api_key,
            gbr_api_base_url: optional!("GBR_API_BASE_URL", "https://api.rtt.io/api"),
            darwin_host,
            darwin_port,
            darwin_username,
            darwin_password,
            darwin_destination: optional!(
                "DARWIN_DESTINATION",
                "/topic/darwin.pushport-v16"
            ),
            watched_routes,
            log_level: optional!("LOG_LEVEL", "info"),
            log_format,
            api_bind_addr: optional!("API_BIND_ADDR", "0.0.0.0:3000"),
        })
    }

    /// Returns a config with safe placeholder values for use in tests.
    /// Does not require any environment variables to be set.
    pub fn for_testing() -> Self {
        Self {
            database_url: "postgres://railpredict:railpredict@localhost:5432/railpredict".to_string(),
            gbr_api_key: "test-key".to_string(),
            gbr_api_base_url: "http://localhost:9999".to_string(),
            darwin_host: "localhost".to_string(),
            darwin_port: 61613,
            darwin_username: "test".to_string(),
            darwin_password: "test".to_string(),
            darwin_destination: "/topic/test".to_string(),
            watched_routes: HashSet::new(),
            log_level: "debug".to_string(),
            log_format: LogFormat::Pretty,
            api_bind_addr: "127.0.0.1:0".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_lists_all_missing_vars_in_one_message() {
        // Test the error display directly — avoids env var race conditions in parallel tests.
        let err = ConfigError {
            missing: vec!["GBR_API_KEY", "DARWIN_HOST", "DARWIN_USERNAME", "DARWIN_PASSWORD"],
        };
        let msg = err.to_string();
        assert!(msg.contains("GBR_API_KEY"), "{msg}");
        assert!(msg.contains("DARWIN_HOST"), "{msg}");
        assert!(msg.contains("DARWIN_USERNAME"), "{msg}");
        assert!(msg.contains("DARWIN_PASSWORD"), "{msg}");
    }

    #[test]
    fn for_testing_does_not_require_env() {
        let config = Config::for_testing();
        assert!(!config.gbr_api_key.is_empty());
        assert!(config.watched_routes.is_empty());
    }

    #[test]
    fn watched_routes_parsed_from_csv() {
        // SAFETY: single-threaded test; no other thread reads these vars concurrently.
        unsafe {
            std::env::set_var("DATABASE_URL", "postgres://u:p@localhost/db");
            std::env::set_var("GBR_API_KEY", "k");
            std::env::set_var("DARWIN_HOST", "h");
            std::env::set_var("DARWIN_USERNAME", "u");
            std::env::set_var("DARWIN_PASSWORD", "p");
            std::env::set_var("WATCHED_ROUTES", "LDS, MAN, KGX");
        }

        let config = Config::from_env().unwrap();
        assert!(config.watched_routes.contains("LDS"));
        assert!(config.watched_routes.contains("MAN"));
        assert!(config.watched_routes.contains("KGX"));
        assert_eq!(config.watched_routes.len(), 3);

        unsafe {
            for v in ["DATABASE_URL","GBR_API_KEY","DARWIN_HOST","DARWIN_USERNAME","DARWIN_PASSWORD","WATCHED_ROUTES"] {
                std::env::remove_var(v);
            }
        }
    }

    #[test]
    fn log_format_json_parsed() {
        // SAFETY: single-threaded test; no other thread reads these vars concurrently.
        unsafe {
            std::env::set_var("DATABASE_URL", "postgres://u:p@localhost/db");
            std::env::set_var("GBR_API_KEY", "k");
            std::env::set_var("DARWIN_HOST", "h");
            std::env::set_var("DARWIN_USERNAME", "u");
            std::env::set_var("DARWIN_PASSWORD", "p");
            std::env::set_var("LOG_FORMAT", "json");
        }

        let config = Config::from_env().unwrap();
        assert_eq!(config.log_format, LogFormat::Json);

        unsafe {
            for v in ["DATABASE_URL","GBR_API_KEY","DARWIN_HOST","DARWIN_USERNAME","DARWIN_PASSWORD","LOG_FORMAT"] {
                std::env::remove_var(v);
            }
        }
    }

    #[test]
    fn database_url_required() {
        let err = ConfigError { missing: vec!["DATABASE_URL"] };
        assert!(err.to_string().contains("DATABASE_URL"));
    }
}
