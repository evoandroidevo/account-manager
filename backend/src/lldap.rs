use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::File, io::BufReader, sync::Arc};

use crate::config::LldapConfig;

/// LDAP result code for a failed bind with wrong credentials.
const LDAP_INVALID_CREDENTIALS: u32 = 49;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LldapUser {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
    pub creation_date: String,
    pub groups: Vec<LldapGroup>,
}

impl LldapUser {
    pub fn is_member_of(&self, group: &str) -> bool {
        self.groups.iter().any(|g| g.display_name == group)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LldapGroup {
    pub id: i32,
    pub display_name: String,
}

#[derive(Deserialize)]
struct GetUserResponse {
    user: LldapUser,
}

#[derive(Clone, Debug)]
pub struct LldapClient {
    pub config: LldapConfig,
    http: reqwest::Client,
}

#[derive(Debug)]
pub enum LldapError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    Graphql(String),
}

impl std::fmt::Display for LldapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(error) => {
                // reqwest's top-level message hides the cause (e.g. a reset connection).
                write!(f, "{error}")?;
                let mut source = std::error::Error::source(error);
                while let Some(cause) = source {
                    write!(f, ": {cause}")?;
                    source = cause.source();
                }
                Ok(())
            }
            Self::Json(error) => write!(f, "{error}"),
            Self::Graphql(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LldapError {}

impl From<reqwest::Error> for LldapError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error)
    }
}

impl From<serde_json::Error> for LldapError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ldap3::LdapError> for LldapError {
    fn from(error: ldap3::LdapError) -> Self {
        Self::Graphql(error.to_string())
    }
}

impl LldapClient {
    pub fn new(config: LldapConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::builder()
                // LLDAP (actix-web) closes idle keep-alive connections after
                // ~5s; reqwest would keep them for 90s and occasionally send a
                // request down one that is being closed. Expire ours first.
                .pool_idle_timeout(std::time::Duration::from_secs(3))
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("static reqwest configuration is valid"),
        }
    }

    pub fn http_url(&self) -> &str {
        &self.config.http_url
    }

    pub fn ldap_url(&self) -> &str {
        &self.config.ldap_url
    }

    pub fn use_tls(&self) -> bool {
        self.config.use_tls
    }

    pub fn tls_insecure_skip_verify(&self) -> bool {
        self.config.tls_insecure_skip_verify
    }

    pub fn tls_ca_file(&self) -> Option<&str> {
        self.config.tls_ca_file.as_deref()
    }

    pub fn base_dn(&self) -> &str {
        &self.config.base_dn
    }

    pub async fn provision_user(
        &self,
        username: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
        password: &str,
        groups: &[String],
    ) -> Result<(), LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;

        self.create_user(&token, username, email, first_name, last_name)
            .await?;

        if let Err(error) = self.set_password(username, password).await {
            let _ = self.delete_user(&token, username).await;
            return Err(error);
        }

        let group_records = match self.list_groups(&token).await {
            Ok(groups) => groups,
            Err(error) => {
                let _ = self.delete_user(&token, username).await;
                return Err(error);
            }
        };

        for group_name in groups {
            let group = group_records
                .iter()
                .find(|group| group.display_name == *group_name)
                .ok_or_else(|| LldapError::Graphql(format!("Unknown group '{group_name}'")))?;

            if let Err(error) = self.add_user_to_group(&token, username, group.id).await {
                let _ = self.delete_user(&token, username).await;
                return Err(error);
            }
        }

        Ok(())
    }

    pub async fn delete_user_account(&self, username: &str) -> Result<(), LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        self.delete_user(&token, username).await
    }

    pub async fn username_exists(&self, username: &str) -> Result<bool, LldapError> {
        self.user_exists("id", username).await
    }

    pub async fn email_exists(&self, email: &str) -> Result<bool, LldapError> {
        self.user_exists("email", email).await
    }

    /// Checks a user's password with an LDAP simple bind. Returns `Ok(false)`
    /// for wrong credentials and `Err` only when LLDAP itself is unreachable.
    pub async fn verify_credentials(
        &self,
        username: &str,
        password: &str,
    ) -> Result<bool, LldapError> {
        // An empty password would turn the bind into an anonymous bind, and an
        // unvalidated username could inject extra DN components.
        if password.is_empty() || crate::validation::validate_username(username).is_err() {
            return Ok(false);
        }

        let settings = self.ldap_settings()?;
        let (connection, mut ldap) =
            ldap3::LdapConnAsync::with_settings(settings, &self.ldap_transport_url()).await?;
        ldap3::drive!(connection);

        let user_dn = format!("uid={},ou=people,{}", username, self.config.base_dn);
        let result = ldap.simple_bind(&user_dn, password).await?;
        let _ = ldap.unbind().await;

        match result.rc {
            0 => Ok(true),
            LDAP_INVALID_CREDENTIALS => Ok(false),
            _ => Err(LldapError::from(result.success().unwrap_err())),
        }
    }

    /// Fetches the live profile and group memberships for a user.
    pub async fn get_user(&self, username: &str) -> Result<Option<LldapUser>, LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        let query = r#"
            query GetUser($id: String!) {
                user(userId: $id) {
                    id email displayName firstName lastName creationDate
                    groups { id displayName }
                }
            }
        "#;
        match self
            .graphql::<GetUserResponse>(&token, query, serde_json::json!({ "id": username }))
            .await
        {
            Ok(response) => Ok(Some(response.user)),
            // LLDAP reports a missing user as a GraphQL error rather than null.
            Err(LldapError::Graphql(message)) if message.contains("not found") => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Sets a new password after confirming the current one.
    /// Returns `Ok(false)` if the current password is wrong.
    pub async fn change_password(
        &self,
        username: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<bool, LldapError> {
        if !self.verify_credentials(username, current_password).await? {
            return Ok(false);
        }
        self.set_password(username, new_password).await?;
        Ok(true)
    }

    pub async fn update_email(&self, username: &str, email: &str) -> Result<(), LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        let query = r#"
            mutation UpdateUser($user: UpdateUserInput!) {
                updateUser(user: $user) { ok }
            }
        "#;
        let response: UpdateUserResponse = self
            .graphql(
                &token,
                query,
                serde_json::json!({ "user": { "id": username, "email": email } }),
            )
            .await?;
        let _ = response.update_user.ok;
        Ok(())
    }

    /// Adds the user to the `add` groups and removes them from the `remove`
    /// groups, by display name. Unknown group names are an error.
    pub async fn update_groups(
        &self,
        username: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(), LldapError> {
        if add.is_empty() && remove.is_empty() {
            return Ok(());
        }
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        let groups = self.list_groups(&token).await?;
        let group_id = |name: &String| {
            groups
                .iter()
                .find(|group| group.display_name == *name)
                .map(|group| group.id)
                .ok_or_else(|| LldapError::Graphql(format!("Unknown group '{name}'")))
        };

        for name in add {
            self.add_user_to_group(&token, username, group_id(name)?)
                .await?;
        }
        let query = r#"
            mutation RemoveUserFromGroup($user: String!, $group: Int!) {
                removeUserFromGroup(userId: $user, groupId: $group) { ok }
            }
        "#;
        for name in remove {
            let response: RemoveUserFromGroupResponse = self
                .graphql(
                    &token,
                    query,
                    serde_json::json!({ "user": username, "group": group_id(name)? }),
                )
                .await?;
            let _ = response.remove_user_from_group.ok;
        }
        Ok(())
    }

    /// Display names of every group in the directory, sorted.
    pub async fn group_names(&self) -> Result<Vec<String>, LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        let mut names: Vec<String> = self
            .list_groups(&token)
            .await?
            .into_iter()
            .map(|group| group.display_name)
            .collect();
        names.sort();
        Ok(names)
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<String, LldapError> {
        let response = self
            .http
            .post(self.endpoint("/auth/simple/login"))
            .json(&LoginRequest { username, password })
            .send()
            .await?
            .error_for_status()?;
        let login = response.json::<LoginResponse>().await?;
        Ok(login.token)
    }

    pub async fn graphql<T: for<'de> Deserialize<'de>>(
        &self,
        token: &str,
        query: &str,
        variables: Value,
    ) -> Result<T, LldapError> {
        let response = self
            .http
            .post(self.endpoint("/api/graphql"))
            .bearer_auth(token)
            .json(&GraphqlRequest { query, variables })
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(LldapError::Graphql(format!(
                "GraphQL request returned {status}: {body}"
            )));
        }

        let response = serde_json::from_str::<GraphqlResponse<T>>(&body)?;
        if let Some(errors) = response.errors {
            return Err(LldapError::Graphql(
                errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }

        response
            .data
            .ok_or_else(|| LldapError::Graphql("GraphQL response had no data".to_owned()))
    }

    async fn create_user(
        &self,
        token: &str,
        username: &str,
        email: &str,
        first_name: &str,
        last_name: &str,
    ) -> Result<(), LldapError> {
        let query = r#"
            mutation CreateUser($user: CreateUserInput!) {
                createUser(user: $user) { id }
            }
        "#;
        let response: CreateUserResponse = self
            .graphql(
                token,
                query,
                serde_json::json!({
                    "user": {
                        "id": username,
                        "email": email,
                        "displayName": format!("{first_name} {last_name}").trim(),
                        "firstName": first_name,
                        "lastName": last_name,
                        "avatar": null,
                        "attributes": null
                    }
                }),
            )
            .await?;
        let _ = response.create_user.id;
        Ok(())
    }

    async fn delete_user(&self, token: &str, username: &str) -> Result<(), LldapError> {
        let query = r#"
            mutation DeleteUser($id: String!) { deleteUser(userId: $id) { ok } }
        "#;
        let response: DeleteUserResponse = self
            .graphql(token, query, serde_json::json!({ "id": username }))
            .await?;
        let _ = response.delete_user.ok;
        Ok(())
    }

    async fn list_groups(&self, token: &str) -> Result<Vec<GroupRecord>, LldapError> {
        let query = r#"
            query GetGroupList { groups { id displayName } }
        "#;
        let response: GroupListResponse = self.graphql(token, query, Value::Null).await?;
        Ok(response.groups)
    }

    async fn user_exists(&self, field: &str, value: &str) -> Result<bool, LldapError> {
        let token = self
            .login(&self.config.username, &self.config.password)
            .await?;
        let query = r#"
            query LookupUsers($filters: RequestFilter) {
                users(filters: $filters) { id }
            }
        "#;
        let response: UserLookupResponse = self
            .graphql(
                &token,
                query,
                serde_json::json!({
                    "filters": {
                        "eq": {
                            "field": field,
                            "value": value,
                        }
                    }
                }),
            )
            .await?;
        Ok(!response.users.is_empty())
    }

    async fn add_user_to_group(
        &self,
        token: &str,
        username: &str,
        group_id: i32,
    ) -> Result<(), LldapError> {
        let query = r#"
            mutation AddUserToGroup($user: String!, $group: Int!) {
                addUserToGroup(userId: $user, groupId: $group) { ok }
            }
        "#;
        let response: AddUserToGroupResponse = self
            .graphql(
                token,
                query,
                serde_json::json!({ "user": username, "group": group_id }),
            )
            .await?;
        let _ = response.add_user_to_group.ok;
        Ok(())
    }

    /// Sets a password as the service account, without checking the old one.
    pub async fn set_password(&self, username: &str, password: &str) -> Result<(), LldapError> {
        let settings = self.ldap_settings()?;
        let (connection, mut ldap) =
            ldap3::LdapConnAsync::with_settings(settings, &self.ldap_transport_url())
                .await
                .map_err(LldapError::from)?;
        ldap3::drive!(connection);

        let admin_dn = format!(
            "cn={},ou=people,{}",
            self.config.username, self.config.base_dn
        );
        let user_dn = format!("uid={},ou=people,{}", username, self.config.base_dn);

        ldap.simple_bind(&admin_dn, &self.config.password)
            .await
            .map_err(LldapError::from)?
            .success()
            .map_err(LldapError::from)?;
        ldap.extended(ldap3::exop::PasswordModify {
            user_id: Some(&user_dn),
            old_pass: None,
            new_pass: Some(password),
        })
        .await
        .map_err(LldapError::from)?
        .success()
        .map_err(LldapError::from)?;
        ldap.unbind().await.map_err(LldapError::from)?;
        Ok(())
    }

    fn ldap_transport_url(&self) -> String {
        if self.config.use_tls {
            self.config.ldap_url.replacen("ldap://", "ldaps://", 1)
        } else {
            self.config.ldap_url.clone()
        }
    }

    fn ldap_settings(&self) -> Result<ldap3::LdapConnSettings, LldapError> {
        let mut settings = ldap3::LdapConnSettings::new();

        if self.config.tls_insecure_skip_verify {
            settings = settings.set_no_tls_verify(true);
        }

        if let Some(ca_file) = &self.config.tls_ca_file {
            let file = File::open(ca_file).map_err(|error| {
                LldapError::Graphql(format!("Unable to open LDAP CA file {ca_file}: {error}"))
            })?;
            let mut reader = BufReader::new(file);
            let certs = rustls_pemfile::certs(&mut reader)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    LldapError::Graphql(format!("Unable to parse LDAP CA file {ca_file}: {error}"))
                })?;

            let mut root_store = rustls::RootCertStore::empty();
            let (valid, invalid) = root_store.add_parsable_certificates(certs);
            if valid == 0 {
                return Err(LldapError::Graphql(format!(
                    "LDAP CA file {ca_file} did not contain any valid certificates"
                )));
            }
            if invalid > 0 {
                return Err(LldapError::Graphql(format!(
                    "LDAP CA file {ca_file} contained {invalid} invalid certificate(s)"
                )));
            }

            let config = rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            settings = settings.set_config(Arc::new(config));
        }

        Ok(settings)
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.http_url.trim_end_matches('/'), path)
    }
}

#[derive(Deserialize)]
struct LoginResponse {
    token: String,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct GraphqlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphqlError>>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Serialize)]
struct GraphqlRequest<'a> {
    query: &'a str,
    variables: Value,
}

#[derive(Deserialize)]
struct CreateUserResponse {
    #[serde(rename = "createUser")]
    create_user: CreatedUser,
}

#[derive(Deserialize)]
struct CreatedUser {
    id: String,
}

#[derive(Deserialize)]
struct DeleteUserResponse {
    #[serde(rename = "deleteUser")]
    delete_user: SuccessValue,
}

#[derive(Deserialize)]
struct UpdateUserResponse {
    #[serde(rename = "updateUser")]
    update_user: SuccessValue,
}

#[derive(Deserialize)]
struct RemoveUserFromGroupResponse {
    #[serde(rename = "removeUserFromGroup")]
    remove_user_from_group: SuccessValue,
}

#[derive(Deserialize)]
struct AddUserToGroupResponse {
    #[serde(rename = "addUserToGroup")]
    add_user_to_group: SuccessValue,
}

#[derive(Deserialize)]
struct SuccessValue {
    ok: bool,
}

#[derive(Deserialize)]
struct GroupListResponse {
    groups: Vec<GroupRecord>,
}

#[derive(Deserialize)]
struct UserLookupResponse {
    users: Vec<UserLookupRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct UserLookupRecord {
    #[serde(rename = "id")]
    _id: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GroupRecord {
    id: i32,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[cfg(test)]
mod tests {
    use super::LldapClient;
    use crate::config::LldapConfig;

    #[test]
    fn exposes_the_configured_endpoints() {
        let client = LldapClient::new(LldapConfig {
            http_url: "http://127.0.0.1:17170".to_owned(),
            ldap_url: "ldap://127.0.0.1:3890".to_owned(),
            use_tls: false,
            tls_insecure_skip_verify: false,
            tls_ca_file: None,
            base_dn: "dc=example,dc=com".to_owned(),
            username: "admin".to_owned(),
            password: "dev-only-password-change-me".to_owned(),
        });

        assert_eq!(client.http_url(), "http://127.0.0.1:17170");
        assert_eq!(client.ldap_url(), "ldap://127.0.0.1:3890");
        assert!(!client.use_tls());
        assert!(!client.tls_insecure_skip_verify());
        assert!(client.tls_ca_file().is_none());
        assert_eq!(client.base_dn(), "dc=example,dc=com");
    }
}
