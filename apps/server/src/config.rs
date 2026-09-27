//! Environment-driven application configuration with strict validation.
//!
//! Rules (STEP-001 §13):
//! - Configuration comes from the environment; nothing is hard-coded per
//!   deployment.
//! - `development` is the only environment with permissive defaults.
//! - `production` fails clearly at startup when required configuration is
//!   missing instead of silently falling back to insecure defaults.

use std::fmt;

/// Deployment environment selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnv {
    Development,
    Test,
    Production,
}

impl AppEnv {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "development" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "production" => Ok(Self::Production),
            other => Err(format!(
                "KOOPERATIF_ENV has invalid value '{other}' \
                 (expected one of: development, test, production)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Production => "production",
        }
    }
}

impl fmt::Display for AppEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validated application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub env: AppEnv,
    pub host: String,
    pub port: u16,
    /// PostgreSQL connection string. Required in `production`; optional in
    /// other environments (the server then runs degraded and `/ready`
    /// reports the database as `unconfigured`).
    pub database_url: Option<String>,
    /// Allowed CORS origins. Empty means same-origin only (no CORS
    /// headers are emitted).
    pub cors_origins: Vec<String>,
}

/// All configuration problems collected at once, reported together.
#[derive(Debug)]
pub struct ConfigError {
    pub problems: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "invalid configuration:")?;
        for problem in &self.problems {
            writeln!(f, "  - {problem}")?;
        }
        writeln!(f, "see apps/server/.env.example for supported variables")
    }
}

impl Config {
    /// Load and validate configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(&|key| std::env::var(key).ok())
    }

    /// Load and validate configuration from a lookup function
    /// (`std::env::var` shaped). Injectable for deterministic tests.
    pub fn from_source(lookup: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut problems: Vec<String> = Vec::new();

        let env = match non_empty(lookup("KOOPERATIF_ENV")) {
            None => AppEnv::Development,
            Some(value) => match AppEnv::parse(&value) {
                Ok(parsed) => parsed,
                Err(problem) => {
                    problems.push(problem);
                    AppEnv::Development
                }
            },
        };

        let host = match non_empty(lookup("KOOPERATIF_HOST")) {
            Some(host) => host,
            None => match env {
                AppEnv::Production => "0.0.0.0".to_string(),
                _ => "127.0.0.1".to_string(),
            },
        };

        let port = match non_empty(lookup("KOOPERATIF_PORT")) {
            None => 8080,
            Some(value) => match value.parse::<u16>() {
                Ok(port) => port,
                Err(_) => {
                    problems.push(format!(
                        "KOOPERATIF_PORT has invalid value '{value}' (expected 1-65535)"
                    ));
                    8080
                }
            },
        };

        let database_url = non_empty(lookup("KOOPERATIF_DATABASE_URL"));
        if env == AppEnv::Production && database_url.is_none() {
            problems.push(
                "KOOPERATIF_DATABASE_URL is required when KOOPERATIF_ENV=production".to_string(),
            );
        }

        let cors_origins = non_empty(lookup("KOOPERATIF_CORS_ORIGINS"))
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|origin| !origin.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }

        Ok(Self {
            env,
            host,
            port,
            database_url,
            cors_origins,
        })
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup_from(map: HashMap<&'static str, String>) -> impl Fn(&str) -> Option<String> {
        move |key| map.get(key).cloned()
    }

    #[test]
    fn development_defaults_apply_without_any_environment() {
        let config = Config::from_source(&lookup_from(HashMap::new())).expect("valid config");
        assert_eq!(config.env, AppEnv::Development);
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert!(config.database_url.is_none());
        assert!(config.cors_origins.is_empty());
    }

    #[test]
    fn production_requires_database_url() {
        let config = Config::from_source(&lookup_from(HashMap::from([(
            "KOOPERATIF_ENV",
            "production".to_string(),
        )])))
        .expect_err("production without database url must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_DATABASE_URL")),
            "error must name the missing variable: {config:?}"
        );
    }

    #[test]
    fn production_accepts_database_url_and_binds_wildcard_behind_proxy() {
        let config = Config::from_source(&lookup_from(HashMap::from([
            ("KOOPERATIF_ENV", "production".to_string()),
            (
                "KOOPERATIF_DATABASE_URL",
                "postgres://user:pass@db:5432/kooperatif".to_string(),
            ),
        ])))
        .expect("valid production config");

        assert_eq!(config.env, AppEnv::Production);
        assert_eq!(config.host, "0.0.0.0");
        assert!(config.database_url.is_some());
    }

    #[test]
    fn invalid_environment_value_is_rejected_with_clear_message() {
        let config = Config::from_source(&lookup_from(HashMap::from([(
            "KOOPERATIF_ENV",
            "staging".to_string(),
        )])))
        .expect_err("invalid env value must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_ENV")
                    && problem.contains("development, test, production")),
            "error must list allowed values: {config:?}"
        );
    }

    #[test]
    fn invalid_port_is_rejected() {
        let config = Config::from_source(&lookup_from(HashMap::from([(
            "KOOPERATIF_PORT",
            "not-a-port".to_string(),
        )])))
        .expect_err("invalid port must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_PORT")),
            "error must name KOOPERATIF_PORT: {config:?}"
        );
    }

    #[test]
    fn cors_origins_parse_from_comma_separated_list() {
        let config = Config::from_source(&lookup_from(HashMap::from([(
            "KOOPERATIF_CORS_ORIGINS",
            "http://localhost:5173 , http://127.0.0.1:5173 ".to_string(),
        )])))
        .expect("valid config");

        assert_eq!(
            config.cors_origins,
            vec![
                "http://localhost:5173".to_string(),
                "http://127.0.0.1:5173".to_string()
            ]
        );
    }
}
