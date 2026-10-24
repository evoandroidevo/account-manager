use std::{str::FromStr, sync::Arc, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

use crate::{
    config::AppConfig,
    lldap::LldapClient,
    mailer::Mailer,
    repo::{
        self, audit::AuditLog, email_changes::EmailChangeRepository,
        entitlements::EntitlementRepository, invites::InviteRepository,
        provisioning::ProvisioningRepository, users::UserRepository, whop::WhopRepository,
    },
    services::Services,
    throttle::LoginThrottle,
    whop::WhopClient,
};

#[derive(Clone, Debug)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub pool: SqlitePool,
    pub invites: InviteRepository,
    pub users: UserRepository,
    pub email_changes: EmailChangeRepository,
    pub entitlements: EntitlementRepository,
    pub provisioning: ProvisioningRepository,
    pub whop: WhopRepository,
    pub audit: AuditLog,
    pub lldap: LldapClient,
    pub mailer: Mailer,
    pub services: Services,
    /// Present when `whop.enabled`.
    pub whop_client: Option<WhopClient>,
    pub login_throttle: LoginThrottle,
}

impl AppState {
    pub async fn initialize(config: AppConfig) -> Result<Self, Box<dyn std::error::Error>> {
        config.validate().map_err(|error| format!("invalid configuration: {error}"))?;
        let options = SqliteConnectOptions::from_str(&config.database.url)?
            .create_if_missing(true)
            .foreign_keys(true)
            // Wait for the write lock instead of failing: a burst of sync
            // jobs must not make a login error out (sqlx's default is 5s).
            .busy_timeout(Duration::from_secs(30));
        if let Some(parent) = options.get_filename().parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        repo::migrate(&pool).await?;

        let login_throttle = LoginThrottle::new(
            config.auth.login_max_attempts,
            Duration::from_secs(config.auth.login_window_minutes * 60),
        );

        Ok(Self {
            invites: InviteRepository::new(pool.clone()),
            users: UserRepository::new(pool.clone()),
            email_changes: EmailChangeRepository::new(pool.clone()),
            entitlements: EntitlementRepository::new(pool.clone()),
            provisioning: ProvisioningRepository::new(pool.clone()),
            services: Services::new(&config),
            whop: WhopRepository::new(pool.clone()),
            whop_client: config.whop.enabled.then(|| WhopClient::new(&config.whop)),
            audit: AuditLog::new(pool.clone()),
            lldap: LldapClient::new(config.ldap.clone()),
            mailer: Mailer::new(&config.mail)?,
            login_throttle,
            config: Arc::new(config),
            pool,
        })
    }
}
