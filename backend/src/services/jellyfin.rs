//! Jellyfin: creates accounts that log in through the LDAP plugin and keeps
//! their user policy in line with the tier.

use rand::{Rng, distr::Alphanumeric};
use serde_json::{Value, json};

use super::{SyncOutcome, check, http_client};
use crate::config::{JellyfinConfig, JellyfinTier};

#[derive(Clone, Debug)]
pub struct JellyfinClient {
    http: reqwest::Client,
    base: String,
    api_key: String,
    auth_provider_id: String,
    password_reset_provider_id: String,
}

impl JellyfinClient {
    pub fn new(config: &JellyfinConfig) -> Self {
        Self {
            http: http_client(),
            base: config.url.trim_end_matches('/').to_owned(),
            api_key: config.api_key.clone(),
            auth_provider_id: config.auth_provider_id.clone(),
            password_reset_provider_id: config.password_reset_provider_id.clone(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .header("Authorization", format!("MediaBrowser Token=\"{}\"", self.api_key))
    }

    /// Makes sure `username` has a Jellyfin account and applies the tier's
    /// policy. Administrators are never changed.
    pub async fn sync(
        &self,
        username: &str,
        tier: Option<&JellyfinTier>,
    ) -> Result<SyncOutcome, String> {
        let user = match self.find_user(username).await? {
            Some(user) => user,
            None => self.create_user(username).await?,
        };
        let id = user["Id"]
            .as_str()
            .ok_or("Jellyfin user has no Id")?
            .to_owned();
        let mut policy = user["Policy"].clone();
        if !policy.is_object() {
            return Err("Jellyfin user has no Policy".to_owned());
        }
        if policy["IsAdministrator"].as_bool() == Some(true) {
            return Ok(SyncOutcome::skipped(
                Some(id),
                "Jellyfin administrator; settings are not managed",
            ));
        }

        if let Some(tier) = tier {
            self.apply_tier(&mut policy, tier).await?;
            let response = self
                .request(reqwest::Method::POST, &format!("/Users/{id}/Policy"))
                .json(&policy)
                .send()
                .await
                .map_err(|error| format!("updating Jellyfin policy failed: {error}"))?;
            check(response, "updating Jellyfin policy").await?;
        }
        Ok(SyncOutcome::done(Some(id)))
    }

    /// Writes the tier's settings into a policy fetched from Jellyfin. Only
    /// those keys change; everything else in the policy is sent back as-is,
    /// because Jellyfin replaces the whole policy on update.
    async fn apply_tier(&self, policy: &mut Value, tier: &JellyfinTier) -> Result<(), String> {
        if let Some(sessions) = tier.max_active_sessions {
            policy["MaxActiveSessions"] = json!(sessions);
        }
        if let Some(mbps) = tier.remote_bitrate_limit_mbps {
            policy["RemoteClientBitrateLimit"] = json!(mbps.saturating_mul(1_000_000));
        }
        if let Some(transcoding) = tier.transcoding {
            policy["EnableVideoPlaybackTranscoding"] = json!(transcoding);
            policy["EnableAudioPlaybackTranscoding"] = json!(transcoding);
            policy["EnablePlaybackRemuxing"] = json!(transcoding);
        }
        if let Some(downloads) = tier.downloads {
            policy["EnableContentDownloading"] = json!(downloads);
        }
        match &tier.libraries {
            None => {
                policy["EnableAllFolders"] = json!(true);
                policy["EnabledFolders"] = json!([]);
            }
            Some(names) => {
                let libraries = self.libraries().await?;
                let mut ids = Vec::new();
                for name in names {
                    let id = libraries
                        .iter()
                        .find(|(library, _)| library.eq_ignore_ascii_case(name))
                        .map(|(_, id)| id.clone())
                        .ok_or_else(|| format!("Jellyfin has no library named \"{name}\""))?;
                    ids.push(id);
                }
                policy["EnableAllFolders"] = json!(false);
                policy["EnabledFolders"] = json!(ids);
            }
        }
        Ok(())
    }

    async fn find_user(&self, username: &str) -> Result<Option<Value>, String> {
        let response = self
            .request(reqwest::Method::GET, "/Users")
            .send()
            .await
            .map_err(|error| format!("listing Jellyfin users failed: {error}"))?;
        let users: Vec<Value> = check(response, "listing Jellyfin users")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Jellyfin users failed: {error}"))?;
        Ok(users.into_iter().find(|user| {
            user["Name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(username))
        }))
    }

    /// Creates the account and points it at the LDAP login provider, so the
    /// user signs in with their LLDAP password. The random local password is
    /// never used.
    async fn create_user(&self, username: &str) -> Result<Value, String> {
        let providers = self.auth_provider_ids().await?;
        if !providers.contains(&self.auth_provider_id) {
            return Err(format!(
                "Jellyfin has no login provider \"{}\" (found: {}). Is the LDAP plugin installed?",
                self.auth_provider_id,
                providers.join(", ")
            ));
        }

        let password: String = rand::rng()
            .sample_iter(&Alphanumeric)
            .take(48)
            .map(char::from)
            .collect();
        let response = self
            .request(reqwest::Method::POST, "/Users/New")
            .json(&json!({ "Name": username, "Password": password }))
            .send()
            .await
            .map_err(|error| format!("creating Jellyfin user failed: {error}"))?;
        let mut user: Value = check(response, "creating Jellyfin user")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading new Jellyfin user failed: {error}"))?;

        let id = user["Id"].as_str().ok_or("new Jellyfin user has no Id")?.to_owned();
        let mut policy = user["Policy"].clone();
        policy["AuthenticationProviderId"] = json!(self.auth_provider_id);
        policy["PasswordResetProviderId"] = json!(self.password_reset_provider_id);
        let response = self
            .request(reqwest::Method::POST, &format!("/Users/{id}/Policy"))
            .json(&policy)
            .send()
            .await
            .map_err(|error| format!("setting Jellyfin login provider failed: {error}"))?;
        check(response, "setting Jellyfin login provider").await?;
        user["Policy"] = policy;
        Ok(user)
    }

    async fn auth_provider_ids(&self) -> Result<Vec<String>, String> {
        let response = self
            .request(reqwest::Method::GET, "/Auth/Providers")
            .send()
            .await
            .map_err(|error| format!("listing Jellyfin login providers failed: {error}"))?;
        let providers: Vec<Value> = check(response, "listing Jellyfin login providers")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Jellyfin login providers failed: {error}"))?;
        Ok(providers
            .iter()
            .filter_map(|provider| provider["Id"].as_str().map(str::to_owned))
            .collect())
    }

    /// (name, id) of every library.
    async fn libraries(&self) -> Result<Vec<(String, String)>, String> {
        let response = self
            .request(reqwest::Method::GET, "/Library/MediaFolders")
            .send()
            .await
            .map_err(|error| format!("listing Jellyfin libraries failed: {error}"))?;
        let body: Value = check(response, "listing Jellyfin libraries")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Jellyfin libraries failed: {error}"))?;
        Ok(body["Items"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|item| {
                Some((item["Name"].as_str()?.to_owned(), item["Id"].as_str()?.to_owned()))
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, header, method, path},
    };

    use super::JellyfinClient;
    use crate::config::{JellyfinConfig, JellyfinTier};

    const LDAP: &str = "Jellyfin.Plugin.LDAP_Auth.LdapAuthenticationProviderPlugin";

    fn client(server: &MockServer) -> JellyfinClient {
        JellyfinClient::new(&JellyfinConfig {
            enabled: true,
            url: server.uri(),
            api_key: "secret".to_owned(),
            ..JellyfinConfig::default()
        })
    }

    fn policy(admin: bool) -> Value {
        json!({
            "IsAdministrator": admin,
            "MaxActiveSessions": 0,
            "EnableAllFolders": true,
            "EnabledFolders": [],
            "BlockedTags": ["keep-me"],
            "AuthenticationProviderId": LDAP,
            "PasswordResetProviderId": "x"
        })
    }

    fn tier() -> JellyfinTier {
        JellyfinTier {
            max_active_sessions: Some(2),
            remote_bitrate_limit_mbps: Some(8),
            transcoding: Some(false),
            downloads: Some(true),
            libraries: Some(vec!["movies".to_owned()]),
        }
    }

    async fn mount_libraries(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/Library/MediaFolders"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Items": [{ "Name": "Movies", "Id": "lib-movies" }, { "Name": "Shows", "Id": "lib-shows" }]
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn applies_the_tier_to_an_existing_user_and_keeps_other_policy_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .and(header("Authorization", "MediaBrowser Token=\"secret\""))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "Name": "Alice", "Id": "u1", "Policy": policy(false) }
            ])))
            .mount(&server)
            .await;
        mount_libraries(&server).await;
        Mock::given(method("POST"))
            .and(path("/Users/u1/Policy"))
            .and(body_partial_json(json!({
                "MaxActiveSessions": 2,
                "RemoteClientBitrateLimit": 8_000_000,
                "EnableVideoPlaybackTranscoding": false,
                "EnableAudioPlaybackTranscoding": false,
                "EnablePlaybackRemuxing": false,
                "EnableContentDownloading": true,
                "EnableAllFolders": false,
                "EnabledFolders": ["lib-movies"],
                "BlockedTags": ["keep-me"],
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = client(&server).sync("alice", Some(&tier())).await.unwrap();
        assert_eq!(outcome.remote_id.as_deref(), Some("u1"));
        assert!(!outcome.skipped);
    }

    #[tokio::test]
    async fn creates_a_missing_user_on_the_ldap_provider() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Auth/Providers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "Name": "Default", "Id": "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider" },
                { "Name": "LDAP-Authentication", "Id": LDAP }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Users/New"))
            .and(body_partial_json(json!({ "Name": "newbie" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Name": "newbie", "Id": "u2",
                "Policy": { "IsAdministrator": false, "AuthenticationProviderId": "default" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Users/u2/Policy"))
            .and(body_partial_json(json!({ "AuthenticationProviderId": LDAP })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let outcome = client(&server).sync("newbie", None).await.unwrap();
        assert_eq!(outcome.remote_id.as_deref(), Some("u2"));
    }

    #[tokio::test]
    async fn refuses_to_create_users_without_the_ldap_provider() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Auth/Providers"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{ "Name": "Default", "Id": "default" }])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/Users/New"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        let error = client(&server).sync("newbie", None).await.unwrap_err();
        assert!(error.contains("LDAP plugin"), "{error}");
    }

    #[tokio::test]
    async fn never_changes_administrators() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "Name": "admin", "Id": "u0", "Policy": policy(true) }
            ])))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;

        let outcome = client(&server).sync("admin", Some(&tier())).await.unwrap();
        assert!(outcome.skipped);
    }

    #[tokio::test]
    async fn reports_unknown_library_names() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Users"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "Name": "alice", "Id": "u1", "Policy": policy(false) }
            ])))
            .mount(&server)
            .await;
        mount_libraries(&server).await;
        let tier = JellyfinTier {
            libraries: Some(vec!["Anime".to_owned()]),
            ..JellyfinTier::default()
        };

        let error = client(&server).sync("alice", Some(&tier)).await.unwrap_err();
        assert!(error.contains("no library named \"Anime\""), "{error}");
    }
}
