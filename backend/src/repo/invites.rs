use sha2::{Digest, Sha256};
use sqlx::{FromRow, SqlitePool};

/// How long expired or revoked invites are kept before `cleanup_stale` removes them.
pub const STALE_INVITE_RETENTION_DAYS: i64 = 120;

pub fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Clone, Debug)]
pub struct InviteRepository {
    pool: SqlitePool,
}

#[derive(Debug, FromRow, PartialEq, Eq)]
pub struct InviteRecord {
    pub id: String,
    pub token_hash: String,
    pub groups_json: String,
    pub created_at: String,
    pub expires_at: String,
    pub consumed_at: Option<String>,
    pub consumed_by: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Debug, FromRow, PartialEq, Eq)]
pub struct InviteEvent {
    pub id: i64,
    pub invite_id: String,
    pub event_type: String,
    pub occurred_at: String,
    pub actor: Option<String>,
    pub metadata_json: Option<String>,
}

impl InviteRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        super::migrate(&self.pool).await
    }

    pub async fn create_invite(
        &self,
        id: &str,
        token_hash: &str,
        groups_json: &str,
        created_at: &str,
        expires_at: &str,
        created_by: Option<&str>,
    ) -> Result<InviteRecord, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO invites
				(id, token_hash, groups_json, created_at, expires_at)
			 VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(token_hash)
        .bind(groups_json)
        .bind(created_at)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;

        insert_event(
            &mut transaction,
            id,
            "created",
            created_at,
            created_by,
            None,
        )
        .await?;

        let invite = sqlx::query_as::<_, InviteRecord>(
            "SELECT id, token_hash, groups_json, created_at, expires_at,
					consumed_at, consumed_by, revoked_at
			 FROM invites WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(invite)
    }

    pub async fn find_by_token_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<InviteRecord>, sqlx::Error> {
        sqlx::query_as::<_, InviteRecord>(
            "SELECT id, token_hash, groups_json, created_at, expires_at,
					consumed_at, consumed_by, revoked_at
			 FROM invites WHERE token_hash = ?",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn consume_invite(
        &self,
        token_hash: &str,
        consumed_at: &str,
        consumed_by: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let invite = sqlx::query_as::<_, InviteRecord>(
            "SELECT id, token_hash, groups_json, created_at, expires_at,
					consumed_at, consumed_by, revoked_at
			 FROM invites
			 WHERE token_hash = ?
			   AND consumed_at IS NULL
			   AND revoked_at IS NULL
			   AND expires_at > ?",
        )
        .bind(token_hash)
        .bind(consumed_at)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(invite) = invite else {
            transaction.rollback().await?;
            return Ok(false);
        };

        sqlx::query("UPDATE invites SET consumed_at = ?, consumed_by = ? WHERE id = ?")
            .bind(consumed_at)
            .bind(consumed_by)
            .bind(&invite.id)
            .execute(&mut *transaction)
            .await?;

        insert_event(
            &mut transaction,
            &invite.id,
            "consumed",
            consumed_at,
            Some(consumed_by),
            None,
        )
        .await?;

        transaction.commit().await?;
        Ok(true)
    }

    pub async fn history(&self, invite_id: &str) -> Result<Vec<InviteEvent>, sqlx::Error> {
        sqlx::query_as::<_, InviteEvent>(
            "SELECT id, invite_id, event_type, occurred_at, actor, metadata_json
			 FROM invite_events WHERE invite_id = ? ORDER BY id",
        )
        .bind(invite_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_recent(&self, limit: i64) -> Result<Vec<InviteRecord>, sqlx::Error> {
        sqlx::query_as::<_, InviteRecord>(
            "SELECT id, token_hash, groups_json, created_at, expires_at,
					consumed_at, consumed_by, revoked_at
			 FROM invites ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn revoke_invite(&self, id: &str, revoked_at: &str) -> Result<bool, sqlx::Error> {
        let mut transaction = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let invite = sqlx::query_as::<_, InviteRecord>(
            "SELECT id, token_hash, groups_json, created_at, expires_at,
					consumed_at, consumed_by, revoked_at
			 FROM invites
			 WHERE id = ?
			   AND consumed_at IS NULL
			   AND revoked_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&mut *transaction)
        .await?;

        let Some(invite) = invite else {
            transaction.rollback().await?;
            return Ok(false);
        };

        sqlx::query("UPDATE invites SET revoked_at = ? WHERE id = ?")
            .bind(revoked_at)
            .bind(&invite.id)
            .execute(&mut *transaction)
            .await?;

        insert_event(
            &mut transaction,
            &invite.id,
            "revoked",
            revoked_at,
            None,
            None,
        )
        .await?;

        transaction.commit().await?;
        Ok(true)
    }

    /// Deletes expired or revoked invites (and their history) whose
    /// `expires_at`/`revoked_at` is older than `cutoff`. Active and consumed
    /// invites are left untouched. Returns the number of invites removed.
    pub async fn cleanup_stale(&self, cutoff: &str) -> Result<u64, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;

        sqlx::query(
            "DELETE FROM invite_events WHERE invite_id IN (
				SELECT id FROM invites
				WHERE (revoked_at IS NOT NULL AND revoked_at < ?)
				   OR (consumed_at IS NULL AND revoked_at IS NULL AND expires_at < ?)
			 )",
        )
        .bind(cutoff)
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;

        let result = sqlx::query(
            "DELETE FROM invites
			 WHERE (revoked_at IS NOT NULL AND revoked_at < ?)
			    OR (consumed_at IS NULL AND revoked_at IS NULL AND expires_at < ?)",
        )
        .bind(cutoff)
        .bind(cutoff)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(result.rows_affected())
    }
}

async fn insert_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    invite_id: &str,
    event_type: &str,
    occurred_at: &str,
    actor: Option<&str>,
    metadata_json: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO invite_events
			(invite_id, event_type, occurred_at, actor, metadata_json)
		 VALUES (?, ?, ?, ?, ?)",
    )
    .bind(invite_id)
    .bind(event_type)
    .bind(occurred_at)
    .bind(actor)
    .bind(metadata_json)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        str::FromStr,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tokio::sync::Barrier;

    use super::InviteRepository;

    fn test_database_url(name: &str) -> String {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{name}-{suffix}.sqlite"));
        path.to_string_lossy().into_owned()
    }

    async fn test_repository(name: &str) -> (InviteRepository, String) {
        let database_path = test_database_url(name);
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{database_path}"))
            .expect("database URL should parse")
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .expect("database should connect");
        let repository = InviteRepository::new(pool);
        repository.migrate().await.expect("migration should run");
        (repository, database_path)
    }

    #[tokio::test]
    async fn creates_and_consumes_an_invite_once_with_history() {
        let (repository, database_path) = test_repository("invite-storage-history").await;

        let invite = repository
            .create_invite(
                "invite-1",
                "hash-1",
                "[\"developers\"]",
                "2026-09-12T10:00:00Z",
                "2026-09-14T10:00:00Z",
                Some("admin"),
            )
            .await
            .expect("invite should be created");
        assert_eq!(invite.id, "invite-1");
        assert!(invite.consumed_at.is_none());

        assert!(
            repository
                .consume_invite("hash-1", "2026-09-12T11:00:00Z", "new-user")
                .await
                .expect("invite should be consumed")
        );
        assert!(
            !repository
                .consume_invite("hash-1", "2026-09-12T12:00:00Z", "another-user")
                .await
                .expect("second consume should be rejected")
        );

        let history = repository
            .history("invite-1")
            .await
            .expect("history should be readable");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].event_type, "created");
        assert_eq!(history[1].event_type, "consumed");

        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn revokes_an_invite_once_and_excludes_it_from_consumption() {
        let (repository, database_path) = test_repository("invite-storage-revoke").await;

        repository
            .create_invite(
                "invite-2",
                "hash-2",
                "[\"developers\"]",
                "2026-09-12T10:00:00Z",
                "2026-09-14T10:00:00Z",
                Some("admin"),
            )
            .await
            .expect("invite should be created");

        assert!(
            repository
                .revoke_invite("invite-2", "2026-09-12T10:30:00Z")
                .await
                .expect("invite should be revoked")
        );
        assert!(
            !repository
                .revoke_invite("invite-2", "2026-09-12T10:45:00Z")
                .await
                .expect("second revoke should be rejected")
        );
        assert!(
            !repository
                .consume_invite("hash-2", "2026-09-12T11:00:00Z", "new-user")
                .await
                .expect("consume should be rejected for a revoked invite")
        );

        let history = repository
            .history("invite-2")
            .await
            .expect("history should be readable");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].event_type, "created");
        assert_eq!(history[1].event_type, "revoked");

        let recent = repository
            .list_recent(10)
            .await
            .expect("recent invites should be listable");
        assert!(recent.iter().any(|invite| invite.id == "invite-2"));

        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn consume_invite_is_atomic_under_concurrency() {
        let (repository, database_path) = test_repository("invite-storage-concurrent").await;

        repository
            .create_invite(
                "invite-3",
                "hash-3",
                "[\"developers\"]",
                "2026-09-12T10:00:00Z",
                "2026-09-14T10:00:00Z",
                Some("admin"),
            )
            .await
            .expect("invite should be created");

        let barrier = Arc::new(Barrier::new(3));
        let repo_a = repository.clone();
        let repo_b = repository.clone();
        let barrier_a = barrier.clone();
        let barrier_b = barrier.clone();

        let task_a = tokio::spawn(async move {
            barrier_a.wait().await;
            repo_a
                .consume_invite("hash-3", "2026-09-12T11:00:00Z", "user-a")
                .await
                .expect("first consume attempt should complete")
        });
        let task_b = tokio::spawn(async move {
            barrier_b.wait().await;
            repo_b
                .consume_invite("hash-3", "2026-09-12T11:00:00Z", "user-b")
                .await
                .expect("second consume attempt should complete")
        });

        barrier.wait().await;
        let (result_a, result_b) = tokio::join!(task_a, task_b);
        let results = [
            result_a.expect("task A should not panic"),
            result_b.expect("task B should not panic"),
        ];
        assert_eq!(results.iter().filter(|result| **result).count(), 1);

        let _ = fs::remove_file(database_path);
    }

    #[tokio::test]
    async fn cleanup_removes_only_stale_expired_and_revoked_invites() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        let repository = InviteRepository::new(pool);
        repository.migrate().await.expect("migration should run");

        // Revoked long ago: should be cleaned up.
        repository
            .create_invite(
                "stale-revoked",
                "hash-stale-revoked",
                "[\"developers\"]",
                "2026-01-01T00:00:00Z",
                "2026-01-03T00:00:00Z",
                None,
            )
            .await
            .expect("invite should be created");
        repository
            .revoke_invite("stale-revoked", "2026-01-02T00:00:00Z")
            .await
            .expect("invite should be revoked");

        // Expired long ago, never consumed or revoked: should be cleaned up.
        repository
            .create_invite(
                "stale-expired",
                "hash-stale-expired",
                "[\"developers\"]",
                "2026-01-01T00:00:00Z",
                "2026-01-03T00:00:00Z",
                None,
            )
            .await
            .expect("invite should be created");

        // Revoked recently: must be kept.
        repository
            .create_invite(
                "recent-revoked",
                "hash-recent-revoked",
                "[\"developers\"]",
                "2026-09-01T00:00:00Z",
                "2026-09-03T00:00:00Z",
                None,
            )
            .await
            .expect("invite should be created");
        repository
            .revoke_invite("recent-revoked", "2026-09-02T00:00:00Z")
            .await
            .expect("invite should be revoked");

        // Consumed long ago: must be kept, cleanup only targets expired/revoked.
        repository
            .create_invite(
                "stale-consumed",
                "hash-stale-consumed",
                "[\"developers\"]",
                "2026-01-01T00:00:00Z",
                "2026-01-03T00:00:00Z",
                None,
            )
            .await
            .expect("invite should be created");
        repository
            .consume_invite("hash-stale-consumed", "2026-01-02T00:00:00Z", "new-user")
            .await
            .expect("invite should be consumed");

        // Still active: must be kept.
        repository
            .create_invite(
                "active",
                "hash-active",
                "[\"developers\"]",
                "2026-09-01T00:00:00Z",
                "2027-09-01T00:00:00Z",
                None,
            )
            .await
            .expect("invite should be created");

        let removed = repository
            .cleanup_stale("2026-05-01T00:00:00Z")
            .await
            .expect("cleanup should run");
        assert_eq!(removed, 2);

        let remaining_ids: Vec<String> = repository
            .list_recent(10)
            .await
            .expect("recent invites should be listable")
            .into_iter()
            .map(|invite| invite.id)
            .collect();
        assert!(!remaining_ids.contains(&"stale-revoked".to_owned()));
        assert!(!remaining_ids.contains(&"stale-expired".to_owned()));
        assert!(remaining_ids.contains(&"recent-revoked".to_owned()));
        assert!(remaining_ids.contains(&"stale-consumed".to_owned()));
        assert!(remaining_ids.contains(&"active".to_owned()));

        assert!(
            repository
                .history("stale-revoked")
                .await
                .expect("history should be readable")
                .is_empty()
        );
    }
}
