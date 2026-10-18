use std::path::Path;

use config::{Config, Environment, File, FileFormat};
use serde::Deserialize;

use crate::validation::PasswordPolicy;

/// Environment variable naming the YAML config file.
pub const CONFIG_PATH_ENV: &str = "APP_CONFIG";
pub const DEFAULT_CONFIG_PATH: &str = "config.yaml";

#[derive(Clone, Debug, Deserialize)]
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
    #[serde(default)]
    pub jellyfin: JellyfinConfig,
    #[serde(default)]
    pub seerr: SeerrConfig,
    /// Tier every account gets unless an admin (or, later, a payment) sets another.
    #[serde(default = "default_tier_id")]
    pub default_tier: String,
    #[serde(default = "default_tiers")]
    pub tiers: Vec<TierConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            ldap: LldapConfig::default(),
            invites: InviteConfig::default(),
            session: SessionConfig::default(),
            auth: AuthConfig::default(),
            mail: MailConfig::default(),
            password_policy: PasswordPolicy::default(),
            jellyfin: JellyfinConfig::default(),
            seerr: SeerrConfig::default(),
            default_tier: default_tier_id(),
            tiers: default_tiers(),
        }
    }
}

impl AppConfig {
    /// Checks cross-field rules that serde can't express. Call once at startup.
    pub fn validate(&self) -> Result<(), String> {
        if self.tiers.is_empty() {
            return Err("at least one tier must be configured".to_owned());
        }
        let mut seen = std::collections::HashSet::new();
        for tier in &self.tiers {
            if tier.id.trim().is_empty() {
                return Err("every tier needs an id".to_owned());
            }
            if !seen.insert(tier.id.as_str()) {
                return Err(format!("tier id \"{}\" is used twice", tier.id));
            }
            if let Some(seerr) = &tier.seerr {
                for name in &seerr.permissions {
                    if crate::services::seerr::permission_bit(name).is_none() {
                        return Err(format!(
                            "tier \"{}\": unknown Seerr permission \"{name}\"",
                            tier.id
                        ));
                    }
                }
            }
        }
        if self.tier(&self.default_tier).is_none() {
            return Err(format!(
                "default_tier \"{}\" is not one of the configured tiers",
                self.default_tier
            ));
        }
        for (name, enabled, url, key) in [
            ("jellyfin", self.jellyfin.enabled, &self.jellyfin.url, &self.jellyfin.api_key),
            ("seerr", self.seerr.enabled, &self.seerr.url, &self.seerr.api_key),
        ] {
            if enabled && (url.trim().is_empty() || key.trim().is_empty()) {
                return Err(format!("{name} is enabled but its url or api_key is empty"));
            }
            if enabled && !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!("{name}.url must start with http:// or https:// (got \"{url}\")"));
            }
        }
        if self.seerr.enabled && !self.jellyfin.enabled {
            return Err("seerr imports users from Jellyfin, so it needs jellyfin enabled".to_owned());
        }
        Ok(())
    }

    pub fn tier(&self, id: &str) -> Option<&TierConfig> {
        self.tiers.iter().find(|tier| tier.id == id)
    }

    /// Every LLDAP group some tier grants. Membership in these is managed by
    /// tier changes; other groups are never touched.
    pub fn tier_managed_groups(&self) -> Vec<String> {
        let mut groups: Vec<String> = self
            .tiers
            .iter()
            .flat_map(|tier| tier.lldap_groups.iter().cloned())
            .collect();
        groups.sort();
        groups.dedup();
        groups
    }
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

fn default_tier_id() -> String {
    "free".to_owned()
}

fn default_tiers() -> Vec<TierConfig> {
    vec![TierConfig {
        id: "free".to_owned(),
        name: "Free".to_owned(),
        description: String::new(),
        price_label: None,
        lldap_groups: Vec::new(),
        jellyfin: None,
        seerr: None,
    }]
}

/// One plan an account can be on, and what it grants in each service.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TierConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Shown on the billing page, e.g. "$5/month".
    #[serde(default)]
    pub price_label: Option<String>,
    /// LLDAP groups members of this tier belong to.
    #[serde(default)]
    pub lldap_groups: Vec<String>,
    /// Omit to leave Jellyfin settings alone for this tier.
    #[serde(default)]
    pub jellyfin: Option<JellyfinTier>,
    /// Omit to leave Seerr settings alone for this tier.
    #[serde(default)]
    pub seerr: Option<SeerrTier>,
}

/// Jellyfin user policy for a tier. Fields left out keep their current value
/// in Jellyfin, except `libraries`, where leaving it out means all libraries.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct JellyfinTier {
    /// Simultaneous streams; 0 means unlimited.
    pub max_active_sessions: Option<i32>,
    /// Cap for streams outside the LAN, in Mbit/s; 0 means unlimited.
    pub remote_bitrate_limit_mbps: Option<i32>,
    /// Allow transcoding (video and audio) and remuxing.
    pub transcoding: Option<bool>,
    pub downloads: Option<bool>,
    /// Library names to give access to; leave out for all libraries.
    pub libraries: Option<Vec<String>>,
}

/// Seerr request limits for a tier.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct SeerrTier {
    /// Leave out to use Seerr's global default quota.
    pub movie_quota: Option<Quota>,
    pub tv_quota: Option<Quota>,
    /// Permission names, e.g. REQUEST, AUTO_APPROVE, REQUEST_4K.
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Quota {
    /// Requests allowed per `days`.
    pub limit: i32,
    pub days: i32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct JellyfinConfig {
    pub enabled: bool,
    /// e.g. http://192.168.1.19:8096
    pub url: String,
    /// Dashboard → API Keys.
    pub api_key: String,
    /// Login provider for accounts this app creates, so they sign in with
    /// their LLDAP password. Check GET /Auth/Providers if unsure.
    pub auth_provider_id: String,
    pub password_reset_provider_id: String,
}

impl Default for JellyfinConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            api_key: String::new(),
            auth_provider_id: "Jellyfin.Plugin.LDAP_Auth.LdapAuthenticationProviderPlugin"
                .to_owned(),
            password_reset_provider_id:
                "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SeerrConfig {
    pub enabled: bool,
    /// e.g. http://seerr:5055
    pub url: String,
    /// Settings → General → API Key.
    pub api_key: String,
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

    #[test]
    fn parses_tiers_and_services() {
        let _guard = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env lock should be available");
        let path = temp_config(
            "account-manager-config-tiers",
            r#"
jellyfin:
  enabled: true
  url: http://jellyfin:8096
  api_key: jf-key
seerr:
  enabled: true
  url: http://seerr:5055
  api_key: seerr-key
default_tier: free
tiers:
  - id: free
    name: Free
    lldap_groups: [user, jelly]
    jellyfin:
      max_active_sessions: 1
      remote_bitrate_limit_mbps: 8
      transcoding: true
      downloads: false
    seerr:
      movie_quota: { limit: 2, days: 7 }
      permissions: [REQUEST]
  - id: plus
    name: Plus
    price_label: "$5/month"
    lldap_groups: [user, jelly, plus]
    jellyfin:
      libraries: [Movies, Shows]
"#,
        );

        let config = AppConfig::load(&path).expect("test config should parse");
        config.validate().expect("config should be valid");

        assert!(config.jellyfin.enabled);
        assert_eq!(config.jellyfin.api_key, "jf-key");
        assert!(config.jellyfin.auth_provider_id.contains("LDAP"));
        assert_eq!(config.seerr.url, "http://seerr:5055");
        assert_eq!(config.tiers.len(), 2);
        let free = config.tier("free").expect("free tier");
        assert_eq!(free.jellyfin.as_ref().unwrap().max_active_sessions, Some(1));
        assert_eq!(free.jellyfin.as_ref().unwrap().libraries, None);
        let quota = free.seerr.as_ref().unwrap().movie_quota.unwrap();
        assert_eq!((quota.limit, quota.days), (2, 7));
        let plus = config.tier("plus").expect("plus tier");
        assert_eq!(plus.price_label.as_deref(), Some("$5/month"));
        assert!(plus.seerr.is_none());
        assert_eq!(config.tier_managed_groups(), ["jelly", "plus", "user"]);

        fs::remove_file(path).expect("test config should be removable");
    }

    #[test]
    fn built_in_defaults_are_valid() {
        let config = AppConfig::default();
        config.validate().expect("defaults should be valid");
        assert_eq!(config.default_tier, "free");
        assert!(!config.jellyfin.enabled);
    }

    #[test]
    fn validation_catches_bad_tier_and_service_setups() {
        use super::{SeerrTier, TierConfig};

        let tier = |id: &str| TierConfig {
            id: id.to_owned(),
            name: id.to_owned(),
            description: String::new(),
            price_label: None,
            lldap_groups: Vec::new(),
            jellyfin: None,
            seerr: None,
        };
        let check = |config: AppConfig, expected: &str| {
            let error = config.validate().expect_err("config should be rejected");
            assert!(error.contains(expected), "{error:?} should mention {expected:?}");
        };

        check(AppConfig { tiers: vec![tier("free"), tier("free")], ..AppConfig::default() }, "used twice");
        check(AppConfig { default_tier: "gold".to_owned(), ..AppConfig::default() }, "default_tier");
        let mut bad_permission = tier("free");
        bad_permission.seerr = Some(SeerrTier { permissions: vec!["FLY".to_owned()], ..SeerrTier::default() });
        check(AppConfig { tiers: vec![bad_permission], ..AppConfig::default() }, "FLY");

        let mut no_key = AppConfig::default();
        no_key.jellyfin.enabled = true;
        no_key.jellyfin.url = "http://jellyfin".to_owned();
        check(no_key, "jellyfin is enabled");

        let mut seerr_alone = AppConfig::default();
        seerr_alone.seerr.enabled = true;
        seerr_alone.seerr.url = "http://seerr".to_owned();
        seerr_alone.seerr.api_key = "key".to_owned();
        check(seerr_alone, "needs jellyfin");

        let mut no_scheme = AppConfig::default();
        no_scheme.jellyfin.enabled = true;
        no_scheme.jellyfin.url = "127.0.0.1:8096".to_owned();
        no_scheme.jellyfin.api_key = "key".to_owned();
        check(no_scheme, "must start with http://");
    }

}
