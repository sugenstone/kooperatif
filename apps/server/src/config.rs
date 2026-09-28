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
    /// Authentication/session subsystem settings (STEP-002, ADR-002).
    pub auth: AuthConfig,
}

/// Authentication and session configuration (ADR-002).
///
/// Security-critical invariants enforced here, at startup:
/// - production requires an explicit Origin allowlist and `Secure`
///   cookies (no silent insecure production defaults);
/// - Argon2 parameters have enforced floors so no environment can
///   accidentally weaken hashing below the documented minimum.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Origins accepted by the CSRF Origin check on state-changing auth
    /// requests. Production must set this explicitly (the public origin).
    pub allowed_origins: Vec<String>,
    /// `Secure` attribute on the session cookie. Forced `true` in
    /// production; development over plain localhost defaults to `false`.
    pub cookie_secure: bool,
    /// Absolute session lifetime in seconds (server-side authority).
    pub session_absolute_ttl_secs: i64,
    /// Idle timeout in seconds, refreshed by bounded session touch.
    pub session_idle_ttl_secs: i64,
    /// Argon2id memory cost in KiB. Default 19456 (19 MiB, OWASP).
    pub argon2_m_cost: u32,
    /// Argon2id time cost (passes). Default 2.
    pub argon2_t_cost: u32,
    /// Argon2id parallelism. Default 1.
    pub argon2_p_cost: u32,
    /// Login rate-limit window in seconds. Windows roll automatically;
    /// there is no permanent account lockout.
    pub login_window_secs: i64,
    /// Max failed logins per normalized username per window.
    pub login_username_max_attempts: u32,
    /// Max login attempts (successes included) per client IP per window.
    pub login_ip_max_attempts: u32,
    /// Trust the rightmost `X-Forwarded-For` entry as client IP. Enable
    /// ONLY behind the reverse proxy of the ADR-012 topology.
    pub forwarded_ip: bool,
}

/// Minimum Argon2 memory cost accepted anywhere (tests may use this;
/// nothing below it is ever accepted).
pub const ARGON2_M_COST_FLOOR: u32 = 8192;
/// Default/production Argon2id parameters (OWASP recommended).
pub const ARGON2_DEFAULT_M_COST: u32 = 19456;
pub const ARGON2_DEFAULT_T_COST: u32 = 2;
pub const ARGON2_DEFAULT_P_COST: u32 = 1;

const SESSION_ABSOLUTE_TTL_DEFAULT_SECS: i64 = 43_200; // 12h
const SESSION_IDLE_TTL_DEFAULT_SECS: i64 = 1_800; // 30min
const LOGIN_WINDOW_DEFAULT_SECS: i64 = 300;
const LOGIN_USERNAME_MAX_ATTEMPTS_DEFAULT: u32 = 10;
const LOGIN_IP_MAX_ATTEMPTS_DEFAULT: u32 = 30;

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

        let auth = auth_config_from_source(lookup, env, &mut problems);

        if !problems.is_empty() {
            return Err(ConfigError { problems });
        }

        Ok(Self {
            env,
            host,
            port,
            database_url,
            cors_origins,
            auth,
        })
    }
}

fn auth_config_from_source(
    lookup: &impl Fn(&str) -> Option<String>,
    env: AppEnv,
    problems: &mut Vec<String>,
) -> AuthConfig {
    // Origin allowlist for the CSRF defense. Development defaults to the
    // Vite dev server; production MUST configure its public origin
    // explicitly — an empty production allowlist would reject every
    // browser login, so it fails fast instead.
    let allowed_origins = match non_empty(lookup("KOOPERATIF_ALLOWED_ORIGINS")) {
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        None => match env {
            AppEnv::Production => Vec::new(),
            _ => vec![
                "http://localhost:5173".to_string(),
                "http://127.0.0.1:5173".to_string(),
            ],
        },
    };
    if env == AppEnv::Production && allowed_origins.is_empty() {
        problems.push(
            "KOOPERATIF_ALLOWED_ORIGINS is required when KOOPERATIF_ENV=production \
             (CSRF Origin validation needs the public origin)"
                .to_string(),
        );
    }

    // Cookie Secure flag: secure by default in production; plain-HTTP
    // development may opt out. Forcing Secure=false in production is a
    // configuration error, never a silent fallback.
    let cookie_secure = match non_empty(lookup("KOOPERATIF_COOKIE_SECURE")) {
        None => env == AppEnv::Production,
        Some(raw) => match raw.as_str() {
            "true" => true,
            "false" => {
                if env == AppEnv::Production {
                    problems.push(
                        "KOOPERATIF_COOKIE_SECURE=false is not allowed in production".to_string(),
                    );
                }
                false
            }
            other => {
                problems.push(format!(
                    "KOOPERATIF_COOKIE_SECURE has invalid value '{other}' (expected true|false)"
                ));
                env == AppEnv::Production
            }
        },
    };

    let session_absolute_ttl_secs = parse_bounded_i64(
        lookup,
        "KOOPERATIF_SESSION_ABSOLUTE_TTL_SECS",
        SESSION_ABSOLUTE_TTL_DEFAULT_SECS,
        60,
        60 * 60 * 24 * 365,
        problems,
    );
    let session_idle_ttl_secs = parse_bounded_i64(
        lookup,
        "KOOPERATIF_SESSION_IDLE_TTL_SECS",
        SESSION_IDLE_TTL_DEFAULT_SECS,
        60,
        60 * 60 * 24 * 365,
        problems,
    );
    if session_idle_ttl_secs >= session_absolute_ttl_secs {
        problems.push(format!(
            "KOOPERATIF_SESSION_IDLE_TTL_SECS ({session_idle_ttl_secs}) must be smaller than \
             KOOPERATIF_SESSION_ABSOLUTE_TTL_SECS ({session_absolute_ttl_secs})"
        ));
    }

    let argon2_m_cost = parse_bounded_u32(
        lookup,
        "KOOPERATIF_ARGON2_M_COST",
        ARGON2_DEFAULT_M_COST,
        ARGON2_M_COST_FLOOR,
        1_048_576,
        problems,
    );
    let argon2_t_cost = parse_bounded_u32(
        lookup,
        "KOOPERATIF_ARGON2_T_COST",
        ARGON2_DEFAULT_T_COST,
        1,
        10,
        problems,
    );
    let argon2_p_cost = parse_bounded_u32(
        lookup,
        "KOOPERATIF_ARGON2_P_COST",
        ARGON2_DEFAULT_P_COST,
        1,
        4,
        problems,
    );

    let login_window_secs = parse_bounded_i64(
        lookup,
        "KOOPERATIF_LOGIN_WINDOW_SECS",
        LOGIN_WINDOW_DEFAULT_SECS,
        1,
        86_400,
        problems,
    );
    let login_username_max_attempts = parse_bounded_u32(
        lookup,
        "KOOPERATIF_LOGIN_USERNAME_MAX_ATTEMPTS",
        LOGIN_USERNAME_MAX_ATTEMPTS_DEFAULT,
        1,
        10_000,
        problems,
    );
    let login_ip_max_attempts = parse_bounded_u32(
        lookup,
        "KOOPERATIF_LOGIN_IP_MAX_ATTEMPTS",
        LOGIN_IP_MAX_ATTEMPTS_DEFAULT,
        1,
        100_000,
        problems,
    );

    let forwarded_ip = match non_empty(lookup("KOOPERATIF_FORWARDED_IP")) {
        None => false,
        Some(raw) => match raw.as_str() {
            "true" => true,
            "false" => false,
            other => {
                problems.push(format!(
                    "KOOPERATIF_FORWARDED_IP has invalid value '{other}' (expected true|false)"
                ));
                false
            }
        },
    };

    AuthConfig {
        allowed_origins,
        cookie_secure,
        session_absolute_ttl_secs,
        session_idle_ttl_secs,
        argon2_m_cost,
        argon2_t_cost,
        argon2_p_cost,
        login_window_secs,
        login_username_max_attempts,
        login_ip_max_attempts,
        forwarded_ip,
    }
}

fn parse_bounded_i64(
    lookup: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
    problems: &mut Vec<String>,
) -> i64 {
    match non_empty(lookup(key)) {
        None => default,
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) if (min..=max).contains(&value) => value,
            Ok(value) => {
                problems.push(format!(
                    "{key} is outside the allowed range {min}..={max}: {value}"
                ));
                default
            }
            Err(_) => {
                problems.push(format!(
                    "{key} has invalid value '{raw}' (expected integer)"
                ));
                default
            }
        },
    }
}

fn parse_bounded_u32(
    lookup: &impl Fn(&str) -> Option<String>,
    key: &str,
    default: u32,
    min: u32,
    max: u32,
    problems: &mut Vec<String>,
) -> u32 {
    match non_empty(lookup(key)) {
        None => default,
        Some(raw) => match raw.parse::<u32>() {
            Ok(value) if (min..=max).contains(&value) => value,
            Ok(value) => {
                problems.push(format!(
                    "{key} is outside the allowed range {min}..={max}: {value}"
                ));
                default
            }
            Err(_) => {
                problems.push(format!(
                    "{key} has invalid value '{raw}' (expected integer)"
                ));
                default
            }
        },
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
            (
                "KOOPERATIF_ALLOWED_ORIGINS",
                "https://kooperatif.example.com".to_string(),
            ),
        ])))
        .expect("valid production config");

        assert_eq!(config.env, AppEnv::Production);
        assert_eq!(config.host, "0.0.0.0");
        assert!(config.database_url.is_some());
        assert_eq!(
            config.auth.allowed_origins,
            vec!["https://kooperatif.example.com".to_string()]
        );
        assert!(
            config.auth.cookie_secure,
            "production cookies must be Secure"
        );
    }

    #[test]
    fn production_requires_allowed_origins_for_csrf_defense() {
        let config = Config::from_source(&lookup_from(HashMap::from([
            ("KOOPERATIF_ENV", "production".to_string()),
            (
                "KOOPERATIF_DATABASE_URL",
                "postgres://user:pass@db:5432/kooperatif".to_string(),
            ),
        ])))
        .expect_err("production without allowed origins must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_ALLOWED_ORIGINS")),
            "error must name KOOPERATIF_ALLOWED_ORIGINS: {config:?}"
        );
    }

    #[test]
    fn development_defaults_serve_local_vite_origin_and_allow_plain_cookie() {
        let config = Config::from_source(&lookup_from(HashMap::new())).expect("valid config");
        assert_eq!(
            config.auth.allowed_origins,
            vec![
                "http://localhost:5173".to_string(),
                "http://127.0.0.1:5173".to_string()
            ]
        );
        assert!(!config.auth.cookie_secure);
        assert_eq!(config.auth.session_absolute_ttl_secs, 43_200);
        assert_eq!(config.auth.session_idle_ttl_secs, 1_800);
        assert_eq!(config.auth.argon2_m_cost, ARGON2_DEFAULT_M_COST);
        assert_eq!(config.auth.argon2_t_cost, ARGON2_DEFAULT_T_COST);
        assert_eq!(config.auth.argon2_p_cost, ARGON2_DEFAULT_P_COST);
        assert_eq!(config.auth.login_window_secs, 300);
        assert!(!config.auth.forwarded_ip);
    }

    #[test]
    fn forcing_insecure_production_cookies_is_rejected() {
        let config = Config::from_source(&lookup_from(HashMap::from([
            ("KOOPERATIF_ENV", "production".to_string()),
            (
                "KOOPERATIF_DATABASE_URL",
                "postgres://user:pass@db:5432/kooperatif".to_string(),
            ),
            (
                "KOOPERATIF_ALLOWED_ORIGINS",
                "https://kooperatif.example.com".to_string(),
            ),
            ("KOOPERATIF_COOKIE_SECURE", "false".to_string()),
        ])))
        .expect_err("insecure production cookie must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_COOKIE_SECURE")),
            "error must name KOOPERATIF_COOKIE_SECURE: {config:?}"
        );
    }

    #[test]
    fn argon2_costs_below_the_floor_are_rejected() {
        let config = Config::from_source(&lookup_from(HashMap::from([(
            "KOOPERATIF_ARGON2_M_COST",
            "1024".to_string(), // insecure; below ARGON2_M_COST_FLOOR
        )])))
        .expect_err("weak argon2 parameters must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("KOOPERATIF_ARGON2_M_COST")),
            "error must name KOOPERATIF_ARGON2_M_COST: {config:?}"
        );
    }

    #[test]
    fn idle_ttl_must_be_smaller_than_absolute_ttl() {
        let config = Config::from_source(&lookup_from(HashMap::from([
            ("KOOPERATIF_SESSION_ABSOLUTE_TTL_SECS", "1800".to_string()),
            ("KOOPERATIF_SESSION_IDLE_TTL_SECS", "1800".to_string()),
        ])))
        .expect_err("idle == absolute must fail");

        assert!(
            config
                .problems
                .iter()
                .any(|problem| problem.contains("IDLE_TTL")),
            "error must name the TTL relationship: {config:?}"
        );
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
