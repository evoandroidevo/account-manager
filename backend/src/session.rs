use serde::{Deserialize, Serialize};

/// Key under which the logged-in user is stored in the tower-sessions record.
pub const SESSION_USER_KEY: &str = "user";

/// What the session remembers about an authenticated user. Everything else
/// (email, groups) is read fresh from LLDAP when needed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionUser {
    pub user_id: String,
    pub username: String,
    pub is_admin: bool,
    /// Unix timestamp (seconds) of the password login that created the session.
    pub authenticated_at: i64,
}

impl SessionUser {
    /// Sessions die after a fixed lifetime even if they stay active; the idle
    /// timeout is enforced separately by the session layer's expiry.
    pub fn is_expired(&self, absolute_timeout_hours: i64, now_unix: i64) -> bool {
        now_unix - self.authenticated_at >= absolute_timeout_hours * 60 * 60
    }
}

#[cfg(test)]
mod tests {
    use super::SessionUser;

    #[test]
    fn expires_after_the_absolute_timeout() {
        let user = SessionUser {
            user_id: "id".to_owned(),
            username: "alice".to_owned(),
            is_admin: false,
            authenticated_at: 1_000,
        };

        assert!(!user.is_expired(1, 1_000 + 3_599));
        assert!(user.is_expired(1, 1_000 + 3_600));
    }
}
