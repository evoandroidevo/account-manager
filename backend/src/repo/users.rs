use sqlx::{FromRow, SqlitePool};

use super::now_rfc3339;

#[derive(Clone, Debug)]
pub struct UserRepository {
    pool: SqlitePool,
}

#[derive(Clone, Debug, FromRow, PartialEq, Eq)]
pub struct UserRecord {
    pub id: String,
    pub username: String,
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub disabled_at: Option<String>,
    /// Unix seconds; sessions authenticated before this are invalid.
    pub password_changed_at: Option<i64>,
    /// The address the user proved they own; LLDAP's may since have changed.
    pub verified_email: Option<String>,
    pub email_verified_at: Option<String>,
}

impl UserRecord {
    /// Whether `email` (the account's address in LLDAP) is the one verified.
    pub fn has_verified(&self, email: &str) -> bool {
        self.verified_email
            .as_deref()
            .is_some_and(|verified| verified.eq_ignore_ascii_case(email))
    }
}

const USER_COLUMNS: &str = "id, username, created_at, last_login_at, disabled_at, \
     password_changed_at, verified_email, email_verified_at";

impl UserRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Returns the local row for an LLDAP user, creating it on first sight.
    /// Accounts created in LLDAP before this app existed get mirrored here
    /// the first time they log in.
    pub async fn ensure(&self, username: &str) -> Result<UserRecord, sqlx::Error> {
        sqlx::query(
            "INSERT INTO users (id, username, created_at) VALUES (?, ?, ?)
             ON CONFLICT (username) DO NOTHING",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(username)
        .bind(now_rfc3339())
        .execute(&self.pool)
        .await?;

        self.find_by_username(username)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    pub async fn find_by_username(&self, username: &str) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!(
            "SELECT {USER_COLUMNS} FROM users WHERE username = ?"
        ))
        .bind(username)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn all(&self) -> Result<Vec<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {USER_COLUMNS} FROM users ORDER BY username"))
            .fetch_all(&self.pool)
            .await
    }

    pub async fn find_by_id(&self, id: &str) -> Result<Option<UserRecord>, sqlx::Error> {
        sqlx::query_as::<_, UserRecord>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn set_password_changed_at(&self, id: &str, unix: i64) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET password_changed_at = ? WHERE id = ?")
            .bind(unix)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn set_verified_email(&self, id: &str, email: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET verified_email = ?, email_verified_at = ? WHERE id = ?")
            .bind(email)
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn touch_login(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::UserRepository;

    async fn repository() -> UserRepository {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory database should connect");
        crate::repo::migrate(&pool).await.expect("migration should run");
        UserRepository::new(pool)
    }

    #[tokio::test]
    async fn ensure_is_idempotent_and_keeps_the_original_id() {
        let users = repository().await;

        let first = users.ensure("alice").await.expect("user should be created");
        let second = users.ensure("alice").await.expect("user should be found");

        assert_eq!(first.id, second.id);
        assert_eq!(second.username, "alice");
        assert!(second.last_login_at.is_none());
    }

    #[tokio::test]
    async fn touch_login_records_the_login_time() {
        let users = repository().await;
        let user = users.ensure("bob").await.expect("user should be created");

        users.touch_login(&user.id).await.expect("login should be recorded");

        let user = users
            .find_by_username("bob")
            .await
            .expect("lookup should work")
            .expect("user should exist");
        assert!(user.last_login_at.is_some());
    }

    #[tokio::test]
    async fn set_verified_email_records_the_address_and_time() {
        let users = repository().await;
        let user = users.ensure("carol").await.expect("user should be created");
        assert!(user.verified_email.is_none());

        users
            .set_verified_email(&user.id, "carol@example.com")
            .await
            .expect("verification should be recorded");

        let user = users
            .find_by_id(&user.id)
            .await
            .expect("lookup should work")
            .expect("user should exist");
        assert_eq!(user.verified_email.as_deref(), Some("carol@example.com"));
        assert!(user.email_verified_at.is_some());
        assert!(user.has_verified("Carol@Example.com"));
        assert!(!user.has_verified("someone@example.com"));
    }
}
