//! Per-service account state and the sync job queue.

use sqlx::{FromRow, SqlitePool};

use super::now_rfc3339;

pub const STATE_PENDING: &str = "pending";
pub const STATE_OK: &str = "ok";
pub const STATE_ERROR: &str = "error";
/// Deliberately not managed (e.g. a Jellyfin administrator).
pub const STATE_SKIPPED: &str = "skipped";

#[derive(Clone, Debug)]
pub struct ProvisioningRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct ServiceAccount {
    pub user_id: String,
    pub service: String,
    pub remote_id: Option<String>,
    pub applied_tier: Option<String>,
    pub state: String,
    pub detail: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct SyncJob {
    pub user_id: String,
    pub service: String,
    pub attempts: i64,
    pub run_after: String,
    pub last_error: Option<String>,
}

const ACCOUNT_COLUMNS: &str = "user_id, service, remote_id, applied_tier, state, detail, updated_at";
const JOB_COLUMNS: &str = "user_id, service, attempts, run_after, last_error";

impl ProvisioningRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Queues a sync to run at `run_after`. An existing job for the same
    /// user and service is reset (attempts cleared, rescheduled) rather than
    /// duplicated, and the account is shown as pending.
    pub async fn enqueue(
        &self,
        user_id: &str,
        service: &str,
        run_after: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_rfc3339();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO sync_jobs (user_id, service, attempts, run_after, created_at)
             VALUES (?, ?, 0, ?, ?)
             ON CONFLICT (user_id, service) DO UPDATE SET
                attempts = 0,
                run_after = excluded.run_after,
                last_error = NULL",
        )
        .bind(user_id)
        .bind(service)
        .bind(run_after)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, service, state, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id, service) DO UPDATE SET
                state = excluded.state,
                detail = NULL,
                updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(service)
        .bind(STATE_PENDING)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }

    pub async fn due_jobs(&self, now: &str, limit: i64) -> Result<Vec<SyncJob>, sqlx::Error> {
        sqlx::query_as::<_, SyncJob>(&format!(
            "SELECT {JOB_COLUMNS} FROM sync_jobs WHERE run_after <= ? ORDER BY run_after LIMIT ?"
        ))
        .bind(now)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn jobs(&self) -> Result<Vec<SyncJob>, sqlx::Error> {
        sqlx::query_as::<_, SyncJob>(&format!("SELECT {JOB_COLUMNS} FROM sync_jobs"))
            .fetch_all(&self.pool)
            .await
    }

    /// Records a finished sync and removes its job.
    pub async fn complete(
        &self,
        user_id: &str,
        service: &str,
        state: &str,
        remote_id: Option<&str>,
        applied_tier: Option<&str>,
        detail: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, service, remote_id, applied_tier, state, detail, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (user_id, service) DO UPDATE SET
                remote_id = COALESCE(excluded.remote_id, service_accounts.remote_id),
                applied_tier = excluded.applied_tier,
                state = excluded.state,
                detail = excluded.detail,
                updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(service)
        .bind(remote_id)
        .bind(applied_tier)
        .bind(state)
        .bind(detail)
        .bind(now_rfc3339())
        .execute(&mut *transaction)
        .await?;
        sqlx::query("DELETE FROM sync_jobs WHERE user_id = ? AND service = ?")
            .bind(user_id)
            .bind(service)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await
    }

    /// Records a failed attempt and schedules the next one.
    pub async fn fail(
        &self,
        user_id: &str,
        service: &str,
        error: &str,
        next_attempt_at: &str,
    ) -> Result<(), sqlx::Error> {
        let now = now_rfc3339();
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE sync_jobs SET attempts = attempts + 1, run_after = ?, last_error = ?
             WHERE user_id = ? AND service = ?",
        )
        .bind(next_attempt_at)
        .bind(error)
        .bind(user_id)
        .bind(service)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, service, state, detail, updated_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (user_id, service) DO UPDATE SET
                state = excluded.state,
                detail = excluded.detail,
                updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(service)
        .bind(STATE_ERROR)
        .bind(error)
        .bind(&now)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }

    pub async fn account(
        &self,
        user_id: &str,
        service: &str,
    ) -> Result<Option<ServiceAccount>, sqlx::Error> {
        sqlx::query_as::<_, ServiceAccount>(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM service_accounts WHERE user_id = ? AND service = ?"
        ))
        .bind(user_id)
        .bind(service)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn accounts_for_user(&self, user_id: &str) -> Result<Vec<ServiceAccount>, sqlx::Error> {
        sqlx::query_as::<_, ServiceAccount>(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM service_accounts WHERE user_id = ?"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn all_accounts(&self) -> Result<Vec<ServiceAccount>, sqlx::Error> {
        sqlx::query_as::<_, ServiceAccount>(&format!("SELECT {ACCOUNT_COLUMNS} FROM service_accounts"))
            .fetch_all(&self.pool)
            .await
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{ProvisioningRepository, STATE_ERROR, STATE_OK, STATE_PENDING};
    use crate::repo::users::UserRepository;

    async fn setup() -> (ProvisioningRepository, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        crate::repo::migrate(&pool).await.expect("migration should run");
        let user = UserRepository::new(pool.clone()).ensure("alice").await.unwrap();
        (ProvisioningRepository::new(pool), user.id)
    }

    #[tokio::test]
    async fn enqueue_coalesces_and_marks_the_account_pending() {
        let (repo, user_id) = setup().await;

        repo.enqueue(&user_id, "jellyfin", "2026-01-01T00:00:10+00:00").await.unwrap();
        repo.fail(&user_id, "jellyfin", "boom", "2026-01-01T01:00:00+00:00").await.unwrap();
        repo.enqueue(&user_id, "jellyfin", "2026-01-01T00:00:20+00:00").await.unwrap();

        let jobs = repo.jobs().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].attempts, 0, "re-enqueueing resets attempts");
        assert_eq!(jobs[0].run_after, "2026-01-01T00:00:20+00:00");
        assert!(jobs[0].last_error.is_none());
        let account = repo.account(&user_id, "jellyfin").await.unwrap().unwrap();
        assert_eq!(account.state, STATE_PENDING);
    }

    #[tokio::test]
    async fn due_jobs_respects_run_after() {
        let (repo, user_id) = setup().await;
        repo.enqueue(&user_id, "lldap", "2026-01-01T00:00:00+00:00").await.unwrap();
        repo.enqueue(&user_id, "seerr", "2026-01-01T02:00:00+00:00").await.unwrap();

        let due = repo.due_jobs("2026-01-01T01:00:00+00:00", 10).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].service, "lldap");
    }

    #[tokio::test]
    async fn fail_then_complete_records_the_outcome() {
        let (repo, user_id) = setup().await;
        repo.enqueue(&user_id, "jellyfin", "2026-01-01T00:00:00+00:00").await.unwrap();

        repo.fail(&user_id, "jellyfin", "unreachable", "2026-01-01T00:01:00+00:00").await.unwrap();
        let account = repo.account(&user_id, "jellyfin").await.unwrap().unwrap();
        assert_eq!(account.state, STATE_ERROR);
        assert_eq!(account.detail.as_deref(), Some("unreachable"));
        assert_eq!(repo.jobs().await.unwrap()[0].attempts, 1);

        repo.complete(&user_id, "jellyfin", STATE_OK, Some("abc"), Some("plus"), None).await.unwrap();
        // A later completion without an id keeps the known one.
        repo.complete(&user_id, "jellyfin", STATE_OK, None, Some("free"), None).await.unwrap();
        let account = repo.account(&user_id, "jellyfin").await.unwrap().unwrap();
        assert_eq!(account.state, STATE_OK);
        assert_eq!(account.remote_id.as_deref(), Some("abc"));
        assert_eq!(account.applied_tier.as_deref(), Some("free"));
        assert!(repo.jobs().await.unwrap().is_empty());
        assert_eq!(repo.accounts_for_user(&user_id).await.unwrap().len(), 1);
    }
}
