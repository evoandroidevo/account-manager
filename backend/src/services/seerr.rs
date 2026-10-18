//! Seerr (Jellyseerr/Overseerr API): imports the user from Jellyfin and sets
//! their request quotas and permissions from the tier.

use serde_json::{Value, json};

use super::{SyncOutcome, check, http_client};
use crate::config::{Quota, SeerrConfig, SeerrTier};

/// Seerr's permission bits (server/lib/permissions.ts).
const PERMISSIONS: &[(&str, u64)] = &[
    ("ADMIN", 2),
    ("MANAGE_USERS", 8),
    ("MANAGE_REQUESTS", 16),
    ("REQUEST", 32),
    ("VOTE", 64),
    ("AUTO_APPROVE", 128),
    ("AUTO_APPROVE_MOVIE", 256),
    ("AUTO_APPROVE_TV", 512),
    ("REQUEST_4K", 1024),
    ("REQUEST_4K_MOVIE", 2048),
    ("REQUEST_4K_TV", 4096),
    ("REQUEST_ADVANCED", 8192),
    ("REQUEST_VIEW", 16384),
    ("AUTO_APPROVE_4K", 32768),
    ("AUTO_APPROVE_4K_MOVIE", 65536),
    ("AUTO_APPROVE_4K_TV", 131072),
    ("REQUEST_MOVIE", 262144),
    ("REQUEST_TV", 524288),
    ("MANAGE_ISSUES", 1048576),
    ("VIEW_ISSUES", 2097152),
    ("CREATE_ISSUES", 4194304),
    ("AUTO_REQUEST", 8388608),
    ("AUTO_REQUEST_MOVIE", 16777216),
    ("AUTO_REQUEST_TV", 33554432),
    ("RECENT_VIEW", 67108864),
    ("WATCHLIST_VIEW", 134217728),
];

pub fn permission_bit(name: &str) -> Option<u64> {
    PERMISSIONS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, bit)| *bit)
}

/// Combined bits for a list of names (validated at startup).
pub fn permission_mask(names: &[String]) -> u64 {
    names.iter().filter_map(|name| permission_bit(name)).fold(0, |mask, bit| mask | bit)
}

/// Seerr's owner account (id 1) can only be changed by itself.
const OWNER_ID: i64 = 1;
/// Accounts with these bits manage Seerr and are never changed.
const ADMIN_BITS: u64 = 2 | 8;

#[derive(Clone, Debug)]
pub struct SeerrClient {
    http: reqwest::Client,
    base: String,
    api_key: String,
}

impl SeerrClient {
    pub fn new(config: &SeerrConfig) -> Self {
        Self {
            http: http_client(),
            base: config.url.trim_end_matches('/').to_owned(),
            api_key: config.api_key.clone(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/api/v1{path}", self.base))
            .header("X-Api-Key", &self.api_key)
    }

    /// Makes sure the Jellyfin user `jellyfin_id` exists in Seerr and applies
    /// the tier's quotas and permissions.
    pub async fn sync(
        &self,
        username: &str,
        jellyfin_id: &str,
        tier: Option<&SeerrTier>,
    ) -> Result<SyncOutcome, String> {
        let user = match self.find_user(username, jellyfin_id).await? {
            Some(user) => user,
            None => self.import_user(username, jellyfin_id).await?,
        };
        let id = user["id"].as_i64().ok_or("Seerr user has no id")?;
        let remote_id = Some(id.to_string());
        let permissions = user["permissions"].as_u64().unwrap_or_default();
        if id == OWNER_ID || permissions & ADMIN_BITS != 0 {
            return Ok(SyncOutcome::skipped(
                remote_id,
                "Seerr administrator; settings are not managed",
            ));
        }

        if let Some(tier) = tier {
            self.set_quotas(id, tier.movie_quota, tier.tv_quota).await?;
            let response = self
                .request(reqwest::Method::POST, &format!("/user/{id}/settings/permissions"))
                .json(&json!({ "permissions": permission_mask(&tier.permissions) }))
                .send()
                .await
                .map_err(|error| format!("setting Seerr permissions failed: {error}"))?;
            check(response, "setting Seerr permissions").await?;
        }
        Ok(SyncOutcome::done(remote_id))
    }

    /// POST /settings/main overwrites username, email and preferences from
    /// the body, so the current settings are read first and sent back with
    /// only the quota fields changed.
    async fn set_quotas(
        &self,
        id: i64,
        movie: Option<Quota>,
        tv: Option<Quota>,
    ) -> Result<(), String> {
        let response = self
            .request(reqwest::Method::GET, &format!("/user/{id}/settings/main"))
            .send()
            .await
            .map_err(|error| format!("reading Seerr user settings failed: {error}"))?;
        let mut settings: Value = check(response, "reading Seerr user settings")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Seerr user settings failed: {error}"))?;

        // A null quota means "use Seerr's global default".
        settings["movieQuotaLimit"] = json!(movie.map(|quota| quota.limit));
        settings["movieQuotaDays"] = json!(movie.map(|quota| quota.days));
        settings["tvQuotaLimit"] = json!(tv.map(|quota| quota.limit));
        settings["tvQuotaDays"] = json!(tv.map(|quota| quota.days));

        let response = self
            .request(reqwest::Method::POST, &format!("/user/{id}/settings/main"))
            .json(&settings)
            .send()
            .await
            .map_err(|error| format!("setting Seerr quotas failed: {error}"))?;
        check(response, "setting Seerr quotas").await?;
        Ok(())
    }

    async fn find_user(&self, username: &str, jellyfin_id: &str) -> Result<Option<Value>, String> {
        let response = self
            .request(reqwest::Method::GET, "/user")
            .query(&[("take", "100"), ("q", username)])
            .send()
            .await
            .map_err(|error| format!("searching Seerr users failed: {error}"))?;
        let body: Value = check(response, "searching Seerr users")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Seerr users failed: {error}"))?;
        let wanted = normalize_id(jellyfin_id);
        Ok(body["results"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .find(|user| {
                user["jellyfinUserId"]
                    .as_str()
                    .is_some_and(|id| normalize_id(id) == wanted)
            })
            .cloned())
    }

    async fn import_user(&self, username: &str, jellyfin_id: &str) -> Result<Value, String> {
        let response = self
            .request(reqwest::Method::POST, "/user/import-from-jellyfin")
            .json(&json!({ "jellyfinUserIds": [jellyfin_id] }))
            .send()
            .await
            .map_err(|error| format!("importing user into Seerr failed: {error}"))?;
        let created: Value = check(response, "importing user into Seerr")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading imported Seerr user failed: {error}"))?;
        if let Some(user) = created.as_array().and_then(|users| users.first()) {
            return Ok(user.clone());
        }
        // Seerr skips ids it already knows; look the user up instead.
        self.find_user(username, jellyfin_id)
            .await?
            .ok_or_else(|| "Seerr did not import the Jellyfin user".to_owned())
    }
}

/// Jellyfin ids show up with and without dashes.
fn normalize_id(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    use super::{SeerrClient, permission_bit, permission_mask};
    use crate::config::{Quota, SeerrConfig, SeerrTier};

    fn client(server: &MockServer) -> SeerrClient {
        SeerrClient::new(&SeerrConfig {
            enabled: true,
            url: server.uri(),
            api_key: "key".to_owned(),
        })
    }

    fn tier() -> SeerrTier {
        SeerrTier {
            movie_quota: Some(Quota { limit: 5, days: 7 }),
            tv_quota: None,
            permissions: vec!["REQUEST".to_owned(), "auto_approve_movie".to_owned()],
        }
    }

    #[test]
    fn permission_names_map_to_seerr_bits() {
        assert_eq!(permission_bit("REQUEST"), Some(32));
        assert_eq!(permission_bit("request_4k"), Some(1024));
        assert_eq!(permission_bit("NOPE"), None);
        assert_eq!(permission_mask(&["REQUEST".to_owned(), "AUTO_APPROVE".to_owned()]), 160);
    }

    #[tokio::test]
    async fn imports_a_new_user_then_sets_quotas_without_losing_other_settings() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/user"))
            .and(query_param("q", "alice"))
            .and(header("X-Api-Key", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pageInfo": { "results": 1 },
                "results": [{ "id": 3, "jellyfinUserId": "someone-else", "permissions": 32 }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/user/import-from-jellyfin"))
            .and(body_json(json!({ "jellyfinUserIds": ["AB-CD"] })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!([
                { "id": 7, "jellyfinUserId": "abcd", "permissions": 32 }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/user/7/settings/main"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "username": "Alice", "email": "alice@example.com", "locale": "en",
                "movieQuotaLimit": null, "movieQuotaDays": null, "tvQuotaLimit": 3, "tvQuotaDays": 7
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/user/7/settings/main"))
            .and(body_json(json!({
                "username": "Alice", "email": "alice@example.com", "locale": "en",
                "movieQuotaLimit": 5, "movieQuotaDays": 7, "tvQuotaLimit": null, "tvQuotaDays": null
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/user/7/settings/permissions"))
            .and(body_json(json!({ "permissions": 32 | 256 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "permissions": 288 })))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = client(&server).sync("alice", "AB-CD", Some(&tier())).await.unwrap();
        assert_eq!(outcome.remote_id.as_deref(), Some("7"));
        assert!(!outcome.skipped);
    }

    #[tokio::test]
    async fn finds_an_existing_user_by_jellyfin_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/user"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{ "id": 4, "jellyfinUserId": "abcd", "permissions": 32 }]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/user/import-from-jellyfin"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;

        let outcome = client(&server).sync("alice", "ab-cd", None).await.unwrap();
        assert_eq!(outcome.remote_id.as_deref(), Some("4"));
    }

    #[tokio::test]
    async fn never_changes_the_owner_or_admins() {
        for (id, permissions) in [(1, 0), (9, 2), (10, 8 | 32)] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/user"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "results": [{ "id": id, "jellyfinUserId": "abcd", "permissions": permissions }]
                })))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;

            let outcome = client(&server).sync("boss", "abcd", Some(&tier())).await.unwrap();
            assert!(outcome.skipped, "user {id} should be skipped");
        }
    }
}
