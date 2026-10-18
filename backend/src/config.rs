use std::path::Path;

use config::{Config, Environment, File, FileFormat};
use serde::Deserialize;

use crate::validation::PasswordPolicy;

/// Environment variable naming the YAML config file.
pub const CONFIG_PATH_ENV: &str = "APP_CONFIG";
pub const DEFAULT_CONFIG_PATH: &str = "config.yaml";

#[derive(Clone, Debug, Default, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub ldap: LldapConfig,
    #[serde(default)]
    pub invites: InviteConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub mail: MailConfig,
    #[serde(default)]
    pub password_policy: PasswordPolicy,
}

impl AppConfig {
    /// Loads an optional YAML file, then applies `APP__SECTION__KEY` environment overrides.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, config::ConfigError> {
        let path = path.as_ref().to_string_lossy();

        Config::builder()
            .add_source(File::new(&path, FileFormat::Yaml).required(false))
            .add_source(
                Environment::with_prefix("APP")
                    .prefix_separator("__")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()?
            .try_deserialize()
    }

    /// Loads from `$APP_CONFIG`, falling back to `./config.yaml`.
    pub fn load_default() -> Result<Self, config::ConfigError> {
        let path =
            std::env::var(CONFIG_PATH_ENV).unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_owned());
        Self::load(path)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Externally visible origin, used for CSRF origin checks and links in emails.
    pub public_url: String,
    /// Trust `X-Forwarded-For` for the client IP (only when behind Caddy).
    pub trust_proxy_headers: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            public_url: "http://127.0.0.1:3000".to_owned(),
            trust_proxy_headers: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://./data/account-manager.db".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LldapConfig {
    pub http_url: String,
    pub ldap_url: String,
    pub use_tls: bool,
    pub tls_insecure_skip_verify: bool,
    pub tls_ca_file: Option<String>,
    pub base_dn: String,
    pub username: String,
    pub password: String,
}

impl Default for LldapConfig {
    fn default() -> Self {
        Self {
            http_url: "http://127.0.0.1:17170".to_owned(),
            ldap_url: "ldap://127.0.0.1:3890".to_owned(),
            use_tls: false,
            tls_insecure_skip_verify: false,
            tls_ca_file: None,
            base_dn: "dc=example,dc=com".to_owned(),
            username: "admin".to_owned(),
            password: "dev-only-password-change-me".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct InviteConfig {
    pub expiration_hours: i64,
    /// Groups pre-selected when an admin creates an invite.
    pub default_groups: Vec<String>,
}

impl Default for InviteConfig {
    fn default() -> Self {
        Self {
            expiration_hours: 48,
            default_groups: vec!["user".to_owned()],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MailTransport {
    /// Send through `mail.smtp`.
    Smtp,
    /// Write each message as an .eml file into `mail.outbox_dir` (dev and tests).
    File,
    /// Only log that a message would have been sent.
    #[default]
    Log,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    pub transport: MailTransport,
    pub from: String,
    pub smtp: SmtpConfig,
    pub outbox_dir: String,
    /// How long an email-change confirmation link stays valid.
    pub email_change_link_hours: i64,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            transport: MailTransport::Log,
            from: "Account Manager <no-reply@example.com>".to_owned(),
            smtp: SmtpConfig::default(),
            outbox_dir: "./data/outbox".to_owned(),
            email_change_link_hours: 24,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    /// STARTTLS on `port` (587). When false, implicit TLS is used (465).
    pub starttls: bool,
    pub username: String,
    pub password: String,
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_owned(),
            port: 587,
            starttls: true,
            username: String::new(),
            password: String::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub cookie_name: String,
    /// Set to false only for local development over plain HTTP.
    pub secure_cookie: bool,
    pub idle_timeout_minutes: i64,
    pub absolute_timeout_hours: i64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cookie_name: "am_session".to_owned(),
            secure_cookie: true,
            idle_timeout_minutes: 60,
            absolute_timeout_hours: 12,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// LLDAP group whose members get the admin pages.
    pub admin_group: String,
    pub login_max_attempts: usize,
    pub login_window_minutes: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            admin_group: "admins".to_owned(),
            login_max_attempts: 5,
            login_window_minutes: 15,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        sync::{Mutex, OnceLock},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::AppConfig;

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    const ENV_KEYS: [&str; 3] = [
        "APP__SESSION__IDLE_TIMEOUT_MINUTES",
        "APP__LDAP__HTTP_URL",
        "APP__AUTH__ADMIN_GROUP",
    ];

    struct EnvGuard(Vec<(&'static str, Option<OsString>)>);

    impl EnvGuard {
        fn capture() -> Self {
            Self(
                ENV_KEYS
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    fn temp_config(name: &str, contents: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{suffix}.yaml"));
        fs::write(&path, contents).expect("test config should be writable");
        path
    }

    #[test]
    fn loads_yaml_values_over_defaults() {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock should be available");
        let _env = EnvGuard::capture();
        for key in ENV_KEYS {
            unsafe { std::env::remove_var(key) };
        }

        let path = temp_config(
            "account-manager-config",
            "server:\n  public_url: https://account.example.com\n  trust_proxy_headers: true\ninvites:\n  expiration_hours: 12\n  default_groups: [user, jelly]\nsession:\n  secure_cookie: false\nmail:\n  transport: smtp\n  smtp:\n    host: mail.example.com\npassword_policy:\n  minimum_length: 16\n",
        );

        let config = AppConfig::load(&path).expect("test config should parse");

        assert_eq!(config.server.public_url, "https://account.example.com");
        assert!(config.server.trust_proxy_headers);
        assert_eq!(config.invites.expiration_hours, 12);
        assert_eq!(config.invites.default_groups, ["user", "jelly"]);
        assert!(!config.session.secure_cookie);
        assert_eq!(config.mail.transport, super::MailTransport::Smtp);
        assert_eq!(config.mail.smtp.host, "mail.example.com");
        assert_eq!(config.mail.smtp.port, 587);
        assert_eq!(config.mail.email_change_link_hours, 24);
        assert_eq!(config.session.idle_timeout_minutes, 60);
        assert_eq!(config.password_policy.minimum_length, 16);
        assert!(config.password_policy.require_uppercase);
        assert_eq!(config.ldap.ldap_url, "ldap://127.0.0.1:3890");
        assert_eq!(config.auth.admin_group, "admins");

        fs::remove_file(path).expect("test config should be removable");
    }

    #[test]
    fn environment_values_override_file_values() {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock should be available");
        let _env = EnvGuard::capture();

        let path = temp_config(
            "account-manager-config-env",
            "ldap:\n  http_url: http://file.example\n  base_dn: dc=file,dc=example\nsession:\n  idle_timeout_minutes: 30\n",
        );

        unsafe {
            std::env::set_var("APP__SESSION__IDLE_TIMEOUT_MINUTES", "5");
            std::env::set_var("APP__LDAP__HTTP_URL", "http://env.example");
            std::env::set_var("APP__AUTH__ADMIN_GROUP", "lldap_admin");
        }

        let config = AppConfig::load(&path).expect("test config should parse");

        assert_eq!(config.session.idle_timeout_minutes, 5);
        assert_eq!(config.ldap.http_url, "http://env.example");
        assert_eq!(config.ldap.base_dn, "dc=file,dc=example");
        assert_eq!(config.auth.admin_group, "lldap_admin");

        fs::remove_file(path).expect("test config should be removable");
    }
}
