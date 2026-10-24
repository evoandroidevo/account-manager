use sqlx::{FromRow, SqlitePool};

use super::now_rfc3339;

#[derive(Clone, Debug)]
pub struct WhopRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct WhopMembership {
    pub membership_id: String,
    pub user_id: Option<String>,
    pub whop_user_id: Option<String>,
    pub email: Option<String>,
    pub plan_id: String,
    pub status: String,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
    /// Set to stop renewing when the current period ends.
    pub cancel_at_period_end: bool,
    pub manage_url: Option<String>,
    pub updated_at: String,
}

/// A downgrade the user scheduled for when their paid membership ends.
#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct PendingDowngrade {
    pub user_id: String,
    pub membership_id: String,
    pub to_tier: String,
    pub created_at: String,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct WebhookEvent {
    pub id: String,
    pub event_type: String,
    pub received_at: String,
    pub membership_id: Option<String>,
    pub outcome: String,
    pub detail: Option<String>,
}

const MEMBERSHIP_COLUMNS: &str = "membership_id, user_id, whop_user_id, email, plan_id, status, \
     current_period_start, current_period_end, cancel_at_period_end, manage_url, updated_at";

impl WhopRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores the latest state of a membership. A known `user_id` is kept
    /// when the new data doesn't carry one.
    pub async fn upsert(&self, membership: &WhopMembership) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO whop_memberships
                (membership_id, user_id, whop_user_id, email, plan_id, status,
                 current_period_start, current_period_end, cancel_at_period_end,
                 manage_url, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (membership_id) DO UPDATE SET
                user_id = COALESCE(excluded.user_id, whop_memberships.user_id),
                whop_user_id = COALESCE(excluded.whop_user_id, whop_memberships.whop_user_id),
                email = COALESCE(excluded.email, whop_memberships.email),
                plan_id = excluded.plan_id,
                status = excluded.status,
                current_period_start = COALESCE(excluded.current_period_start, whop_memberships.current_period_start),
                current_period_end = excluded.current_period_end,
                cancel_at_period_end = excluded.cancel_at_period_end,
                manage_url = COALESCE(excluded.manage_url, whop_memberships.manage_url),
                updated_at = excluded.updated_at",
        )
        .bind(&membership.membership_id)
        .bind(&membership.user_id)
        .bind(&membership.whop_user_id)
        .bind(&membership.email)
        .bind(&membership.plan_id)
        .bind(&membership.status)
        .bind(&membership.current_period_start)
        .bind(&membership.current_period_end)
        .bind(membership.cancel_at_period_end)
        .bind(&membership.manage_url)
        .bind(now_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn find(&self, membership_id: &str) -> Result<Option<WhopMembership>, sqlx::Error> {
        sqlx::query_as::<_, WhopMembership>(&format!(
            "SELECT {MEMBERSHIP_COLUMNS} FROM whop_memberships WHERE membership_id = ?"
        ))
        .bind(membership_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn for_user(&self, user_id: &str) -> Result<Vec<WhopMembership>, sqlx::Error> {
        sqlx::query_as::<_, WhopMembership>(&format!(
            "SELECT {MEMBERSHIP_COLUMNS} FROM whop_memberships WHERE user_id = ? ORDER BY updated_at DESC"
        ))
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Memberships not yet matched to an account.
    pub async fn unmatched(&self) -> Result<Vec<WhopMembership>, sqlx::Error> {
        sqlx::query_as::<_, WhopMembership>(&format!(
            "SELECT {MEMBERSHIP_COLUMNS} FROM whop_memberships WHERE user_id IS NULL ORDER BY updated_at DESC"
        ))
        .fetch_all(&self.pool)
        .await
    }

    /// Links a membership to an account. Returns false if it doesn't exist.
    pub async fn assign(&self, membership_id: &str, user_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE whop_memberships SET user_id = ?, updated_at = ? WHERE membership_id = ?",
        )
        .bind(user_id)
        .bind(now_rfc3339())
        .bind(membership_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn set_cancel_at_period_end(
        &self,
        membership_id: &str,
        cancel: bool,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE whop_memberships SET cancel_at_period_end = ?, updated_at = ? WHERE membership_id = ?",
        )
        .bind(cancel)
        .bind(now_rfc3339())
        .bind(membership_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_pending_downgrade(
        &self,
        user_id: &str,
        membership_id: &str,
        to_tier: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO pending_downgrades (user_id, membership_id, to_tier, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT (user_id) DO UPDATE SET
                membership_id = excluded.membership_id,
                to_tier = excluded.to_tier,
                created_at = excluded.created_at",
        )
        .bind(user_id)
        .bind(membership_id)
        .bind(to_tier)
        .bind(now_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_downgrade(&self, user_id: &str) -> Result<Option<PendingDowngrade>, sqlx::Error> {
        sqlx::query_as::<_, PendingDowngrade>(
            "SELECT user_id, membership_id, to_tier, created_at FROM pending_downgrades WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn clear_pending_downgrade(&self, user_id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM pending_downgrades WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Claims a webhook delivery. Returns false if this id was already
    /// received, so retries are only processed once.
    pub async fn claim_event(&self, id: &str, event_type: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "INSERT OR IGNORE INTO webhook_events (id, event_type, received_at, outcome)
             VALUES (?, ?, ?, 'received')",
        )
        .bind(id)
        .bind(event_type)
        .bind(now_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn finish_event(
        &self,
        id: &str,
        membership_id: Option<&str>,
        outcome: &str,
        detail: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE webhook_events SET membership_id = ?, outcome = ?, detail = ? WHERE id = ?",
        )
        .bind(membership_id)
        .bind(outcome)
        .bind(detail)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Lets a delivery be processed again (e.g. after an internal error, so
    /// Whop's retry isn't dropped as a duplicate).
    pub async fn release_event(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM webhook_events WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn recent_events(&self, limit: i64) -> Result<Vec<WebhookEvent>, sqlx::Error> {
        sqlx::query_as::<_, WebhookEvent>(
            "SELECT id, event_type, received_at, membership_id, outcome, detail
             FROM webhook_events ORDER BY received_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{WhopMembership, WhopRepository};
    use crate::repo::users::UserRepository;

    async fn setup() -> (WhopRepository, String) {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        crate::repo::migrate(&pool).await.expect("migration should run");
        let user = UserRepository::new(pool.clone()).ensure("alice").await.unwrap();
        (WhopRepository::new(pool), user.id)
    }

    fn membership(user_id: Option<&str>, status: &str) -> WhopMembership {
        WhopMembership {
            membership_id: "mem_1".to_owned(),
            user_id: user_id.map(str::to_owned),
            whop_user_id: Some("user_1".to_owned()),
            email: Some("alice@example.com".to_owned()),
            plan_id: "plan_plus".to_owned(),
            status: status.to_owned(),
            current_period_start: Some("2026-11-01T00:00:00+00:00".to_owned()),
            current_period_end: Some("2026-12-01T00:00:00+00:00".to_owned()),
            cancel_at_period_end: false,
            manage_url: Some("https://whop.com/billing/manage/mem_1".to_owned()),
            updated_at: String::new(),
        }
    }

    #[tokio::test]
    async fn upsert_keeps_a_known_user_and_tracks_unmatched() {
        let (repo, user_id) = setup().await;

        repo.upsert(&membership(None, "active")).await.unwrap();
        assert_eq!(repo.unmatched().await.unwrap().len(), 1);

        assert!(repo.assign("mem_1", &user_id).await.unwrap());
        assert!(!repo.assign("mem_unknown", &user_id).await.unwrap());
        // A later update without a user keeps the assignment.
        repo.upsert(&membership(None, "canceled")).await.unwrap();

        let stored = repo.find("mem_1").await.unwrap().unwrap();
        assert_eq!(stored.user_id.as_deref(), Some(user_id.as_str()));
        assert_eq!(stored.status, "canceled");
        assert!(repo.unmatched().await.unwrap().is_empty());
        assert_eq!(repo.for_user(&user_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tracks_scheduled_cancellation_and_pending_downgrades() {
        let (repo, user_id) = setup().await;
        repo.upsert(&membership(Some(&user_id), "active")).await.unwrap();

        repo.set_cancel_at_period_end("mem_1", true).await.unwrap();
        assert!(repo.find("mem_1").await.unwrap().unwrap().cancel_at_period_end);

        repo.set_pending_downgrade(&user_id, "mem_1", "plus").await.unwrap();
        repo.set_pending_downgrade(&user_id, "mem_1", "free").await.unwrap();
        assert_eq!(repo.pending_downgrade(&user_id).await.unwrap().unwrap().to_tier, "free");
        repo.clear_pending_downgrade(&user_id).await.unwrap();
        assert!(repo.pending_downgrade(&user_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn webhook_deliveries_are_claimed_once() {
        let (repo, _) = setup().await;

        assert!(repo.claim_event("msg_1", "membership.activated").await.unwrap());
        assert!(!repo.claim_event("msg_1", "membership.activated").await.unwrap());
        repo.finish_event("msg_1", Some("mem_1"), "applied", None).await.unwrap();

        let events = repo.recent_events(10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].outcome, "applied");

        repo.release_event("msg_1").await.unwrap();
        assert!(repo.claim_event("msg_1", "membership.activated").await.unwrap());
    }
}
