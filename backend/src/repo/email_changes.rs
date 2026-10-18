use sqlx::{FromRow, SqlitePool};

use super::now_rfc3339;

#[derive(Clone, Debug)]
pub struct EmailChangeRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct EmailChangeRecord {
    pub id: String,
    pub user_id: String,
    pub new_email: String,
    pub token_hash: String,
    pub created_at: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
}

impl EmailChangeRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores a new pending change, replacing any unconfirmed one for the
    /// same user so only the most recent link works.
    pub async fn create(
        &self,
        user_id: &str,
        new_email: &str,
        token_hash: &str,
        expires_at: &str,
    ) -> Result<EmailChangeRecord, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query("DELETE FROM email_changes WHERE user_id = ? AND consumed_at IS NULL")
            .bind(user_id)
            .execute(&mut *transaction)
            .await?;

        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO email_changes (id, user_id, new_email, token_hash, created_at, expires_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user_id)
        .bind(new_email)
        .bind(token_hash)
        .bind(now_rfc3339())
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;

        let record = sqlx::query_as::<_, EmailChangeRecord>(
            "SELECT id, user_id, new_email, token_hash, created_at, expires_at, consumed_at
             FROM email_changes WHERE id = ?",
        )
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(record)
    }

    /// An unconfirmed, unexpired change for this token.
    pub async fn find_active(
        &self,
        token_hash: &str,
        now: &str,
    ) -> Result<Option<EmailChangeRecord>, sqlx::Error> {
        sqlx::query_as::<_, EmailChangeRecord>(
            "SELECT id, user_id, new_email, token_hash, created_at, expires_at, consumed_at
             FROM email_changes
             WHERE token_hash = ? AND consumed_at IS NULL AND expires_at > ?",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
    }

    /// Marks the change used. Returns false if it was already used, so a
    /// link can't be applied twice even under concurrent clicks.
    pub async fn consume(&self, id: &str, now: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE email_changes SET consumed_at = ? WHERE id = ? AND consumed_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Undoes `consume` when applying the change failed, so the link can be retried.
    pub async fn release(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE email_changes SET consumed_at = NULL WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::EmailChangeRepository;
    use crate::repo::users::UserRepository;

    async fn setup() -> (EmailChangeRepository, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        crate::repo::migrate(&pool).await.expect("migration should run");
        let user = UserRepository::new(pool.clone())
            .ensure("alice")
            .await
            .expect("user should be created");
        (EmailChangeRepository::new(pool), user.id)
    }

    #[tokio::test]
    async fn a_new_request_replaces_the_pending_one() {
        let (changes, user_id) = setup().await;

        changes
            .create(&user_id, "old-request@example.com", "hash-1", "2999-01-01T00:00:00Z")
            .await
            .expect("first change should be stored");
        changes
            .create(&user_id, "new-request@example.com", "hash-2", "2999-01-01T00:00:00Z")
            .await
            .expect("second change should be stored");

        let now = "2026-01-01T00:00:00Z";
        assert!(changes.find_active("hash-1", now).await.unwrap().is_none());
        let active = changes.find_active("hash-2", now).await.unwrap().expect("active");
        assert_eq!(active.new_email, "new-request@example.com");
    }

    #[tokio::test]
    async fn expired_changes_are_not_active() {
        let (changes, user_id) = setup().await;
        changes
            .create(&user_id, "a@example.com", "hash", "2026-01-01T00:00:00Z")
            .await
            .unwrap();

        assert!(changes.find_active("hash", "2026-01-02T00:00:00Z").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_change_can_be_consumed_once_and_released() {
        let (changes, user_id) = setup().await;
        let record = changes
            .create(&user_id, "a@example.com", "hash", "2999-01-01T00:00:00Z")
            .await
            .unwrap();
        let now = "2026-01-01T00:00:00Z";

        assert!(changes.consume(&record.id, now).await.unwrap());
        assert!(!changes.consume(&record.id, now).await.unwrap());
        assert!(changes.find_active("hash", now).await.unwrap().is_none());

        changes.release(&record.id).await.unwrap();
        assert!(changes.find_active("hash", now).await.unwrap().is_some());
    }
}
