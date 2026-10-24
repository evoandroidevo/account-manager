pub mod audit;
pub mod email_changes;
pub mod entitlements;
pub mod invites;
pub mod provisioning;
pub mod users;
pub mod whop;

use sqlx::SqlitePool;

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
