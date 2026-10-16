use sqlx::SqlitePool;

use super::now_rfc3339;

/// Append-only record of security-relevant actions (logins, password and
/// email changes, tier changes).
#[derive(Clone, Debug)]
pub struct AuditLog {
    pool: SqlitePool,
}

impl AuditLog {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record(
        &self,
        actor: Option<&str>,
        action: &str,
        target: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO audit_log (occurred_at, actor, action, target, metadata_json)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(now_rfc3339())
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(metadata.map(|value| value.to_string()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Like `record`, but failures are only logged so they never break the
    /// user-facing request that triggered them.
    pub async fn record_best_effort(
        &self,
        actor: Option<&str>,
        action: &str,
        target: Option<&str>,
        metadata: Option<serde_json::Value>,
    ) {
        if let Err(error) = self.record(actor, action, target, metadata).await {
            log::error!("failed to write audit log entry {action}: {error}");
        }
    }
}
