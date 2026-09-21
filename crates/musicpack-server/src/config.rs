//! Server configuration — the port of the reference's `config.c`.
//!
//! Precedence: command line > environment > defaults. Defaults are safe:
//! loopback-only binding, no remote access implied. Environment variables
//! (`MUSICPACK_LIBRARY` / `MUSICPACK_DATABASE` / `MUSICPACK_LISTEN` /
//! `MUSICPACK_PORT`) are applied only when set and non-empty, exactly like
//! the reference.

/// Maximum number of explicitly allowed CORS origins (`MP_ALLOW_ORIGIN_MAX`).
pub const ALLOW_ORIGIN_MAX: usize = 8;

/// The server configuration (`mp_config`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Music root (`./library`).
    pub library: String,
    /// SQLite database path (`./library.db`).
    pub database: String,
    /// Bind address (`127.0.0.1`).
    pub listen: String,
    /// Listen port (`8080`).
    pub port: u16,
    /// scan/verify: full SHA-256 integrity verification.
    pub verify_on_scan: bool,
    /// serve: skip the startup scan.
    pub no_scan: bool,
    /// serve: static file root (empty = disabled).
    pub static_dir: String,
    /// Explicit CORS whitelist (denied unless listed; max
    /// [`ALLOW_ORIGIN_MAX`]).
    pub allow_origin: Vec<String>,
    /// Session cookies always carry the Secure flag.
    pub secure_cookies: bool,
    /// serve: optional graceful-shutdown trigger file (empty = disabled).
    /// When the file appears the server drains and exits (R4.5).
    pub shutdown_file: String,
}

// Several fields are consumed only by later migration stages (serve,
// scanner); they are parsed now so the CLI surface and precedence rules are
// final from day one.
#[allow(dead_code)]
impl Config {
    /// The reference defaults (`mp_config_defaults`).
    pub fn defaults() -> Self {
        Self {
            library: "./library".into(),
            database: "./library.db".into(),
            listen: "127.0.0.1".into(),
            port: 8080,
            verify_on_scan: false,
            no_scan: false,
            static_dir: String::new(),
            allow_origin: Vec::new(),
            secure_cookies: false,
            shutdown_file: String::new(),
        }
    }

    /// Applies the environment layer (`mp_config_apply_env`) using `lookup`,
    /// so precedence is testable without process-global mutation.
    pub fn apply_env_with(mut self, lookup: &dyn Fn(&str) -> Option<String>) -> Self {
        // Empty values are ignored, exactly like the reference.
        if let Some(v) = non_empty(lookup, "MUSICPACK_LIBRARY") {
            self.library = v;
        }
        if let Some(v) = non_empty(lookup, "MUSICPACK_DATABASE") {
            self.database = v;
        }
        if let Some(v) = non_empty(lookup, "MUSICPACK_LISTEN") {
            self.listen = v;
        }
        if let Some(v) = non_empty(lookup, "MUSICPACK_PORT") {
            // The reference uses atoi(); garbage parses as 0 and is rejected
            // by the same `invalid port` check either way.
            self.port = v.parse().unwrap_or(0);
        }
        // R4.5 additive ops setting (not in the C): the graceful-shutdown
        // trigger file. Empty/unset keeps it disabled, so the C-visible
        // configuration contract is unchanged.
        if let Some(v) = non_empty(lookup, "MUSICPACK_SHUTDOWN_FILE") {
            self.shutdown_file = v;
        }
        self
    }

    /// Applies the environment layer from the real process environment.
    pub fn apply_env(self) -> Self {
        self.apply_env_with(&|key| std::env::var(key).ok())
    }

    /// Adds a CORS origin, rejecting empties and overflow (`mp_config_add_origin`).
    /// Returns `false` when the origin was not added.
    pub fn add_origin(&mut self, origin: &str) -> bool {
        if origin.is_empty() || self.allow_origin.len() >= ALLOW_ORIGIN_MAX {
            return false;
        }
        self.allow_origin.push(origin.to_string());
        true
    }
}

fn non_empty(lookup: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    lookup(key).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_reference() {
        let c = Config::defaults();
        assert_eq!(c.library, "./library");
        assert_eq!(c.database, "./library.db");
        assert_eq!(c.listen, "127.0.0.1");
        assert_eq!(c.port, 8080);
        assert!(!c.verify_on_scan);
        assert!(!c.no_scan);
        assert_eq!(c.static_dir, "");
        assert!(c.allow_origin.is_empty());
        assert!(!c.secure_cookies);
        assert_eq!(c.shutdown_file, "", "graceful shutdown disabled by default");
    }

    #[test]
    fn env_layer_overrides_defaults_and_ignores_empty_values() {
        let c = Config::defaults().apply_env_with(&|key| match key {
            "MUSICPACK_LIBRARY" => Some("/srv/music".into()),
            "MUSICPACK_PORT" => Some("9000".into()),
            "MUSICPACK_DATABASE" => Some(String::new()), // empty: ignored
            "MUSICPACK_SHUTDOWN_FILE" => Some("/run/musicpack.stop".into()),
            _ => None,
        });
        assert_eq!(c.library, "/srv/music");
        assert_eq!(c.port, 9000);
        assert_eq!(c.database, "./library.db");
        assert_eq!(c.shutdown_file, "/run/musicpack.stop");
    }

    #[test]
    fn garbage_port_parses_as_zero_and_is_rejected_later() {
        let c = Config::defaults().apply_env_with(&|key| match key {
            "MUSICPACK_PORT" => Some("not-a-number".into()),
            _ => None,
        });
        assert_eq!(c.port, 0);
    }

    #[test]
    fn origin_whitelist_is_capped() {
        let mut c = Config::defaults();
        assert!(c.add_origin("https://a.example"));
        assert!(!c.add_origin(""), "empty origin rejected");
        for i in 1..ALLOW_ORIGIN_MAX {
            assert!(c.add_origin(&format!("https://h{i}.example")));
        }
        assert_eq!(c.allow_origin.len(), ALLOW_ORIGIN_MAX);
        assert!(!c.add_origin("https://overflow.example"), "cap enforced");
    }
}
