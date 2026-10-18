use sqlx::{FromRow, SqlitePool};

use super::now_rfc3339;

/// Set by an admin on the users page.
pub const SOURCE_MANUAL: &str = "manual";
/// Set by a paid Whop membership (phase 4).
pub const SOURCE_WHOP: &str = "whop";

#[derive(Clone, Debug)]
pub struct EntitlementRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct Entitlement {
    pub user_id: String,
    pub source: String,
    pub tier: String,
    /// RFC 3339; `None` means it doesn't expire.
    pub valid_until: Option<String>,
    pub note: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

const COLUMNS: &str = "user_id, source, tier, valid_until, note, updated_at, updated_by";

impl EntitlementRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Creates or replaces the user's entitlement from `source`.
    pub async fn set(
        &self,
        user_id: &str,
        source: &str,
        tier: &str,
        valid_until: Option<&str>,
        note: Option<&str>,
        updated_by: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO entitlements (user_id, source, tier, valid_until, note, updated_at, updated_by)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id, source) DO UPDATE SET
                tier = excluded.tier,
                valid_until = excluded.valid_until,
                note = excluded.note,
                updated_at = excluded.updated_at,
                updated_by = excluded.updated_by",
        )
        .bind(user_id)
        .bind(source)
        .bind(tier)
        .bind(valid_until)
        .bind(note)
        .bind(now_rfc3339())
        .bind(updated_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns false if there was nothing to remove.
    pub async fn clear(&self, user_id: &str, source: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM entitlements WHERE user_id = ? AND source = ?")
            .bind(user_id)
            .bind(source)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn for_user(&self, user_id: &str) -> Result<Vec<Entitlement>, sqlx::Error> {
        sqlx::query_as::<_, Entitlement>(&format!(
            "SELECT {COLUMNS} FROM entitlements WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn all(&self) -> Result<Vec<Entitlement>, sqlx::Error> {
        sqlx::query_as::<_, Entitlement>(&format!("SELECT {COLUMNS} FROM entitlements"))
            .fetch_all(&self.pool)
            .await
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{EntitlementRepository, SOURCE_MANUAL, SOURCE_WHOP};
    use crate::repo::users::UserRepository;

    #[tokio::test]
    async fn set_replaces_per_source_and_clear_removes() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        crate::repo::migrate(&pool).await.expect("migration should run");
        let user = UserRepository::new(pool.clone()).ensure("alice").await.unwrap();
        let entitlements = EntitlementRepository::new(pool);

        entitlements
            .set(&user.id, SOURCE_MANUAL, "plus", None, None, Some("bob"))
            .await
            .unwrap();
        entitlements
            .set(&user.id, SOURCE_MANUAL, "premium", Some("2027-01-01T00:00:00+00:00"), Some("gift"), Some("bob"))
            .await
            .unwrap();
        entitlements
            .set(&user.id, SOURCE_WHOP, "plus", None, None, None)
            .await
            .unwrap();

        let mut rows = entitlements.for_user(&user.id).await.unwrap();
        rows.sort_by(|a, b| a.source.cmp(&b.source));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, SOURCE_MANUAL);
        assert_eq!(rows[0].tier, "premium");
        assert_eq!(rows[0].note.as_deref(), Some("gift"));

        assert!(entitlements.clear(&user.id, SOURCE_MANUAL).await.unwrap());
        assert!(!entitlements.clear(&user.id, SOURCE_MANUAL).await.unwrap());
        assert_eq!(entitlements.all().await.unwrap().len(), 1);
    }
}
