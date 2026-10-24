//! Whop payments: checkout links for paid tiers, verified membership
//! webhooks, and a reconcile that repairs anything a webhook missed.
//!
//! A user's paid tier comes from their best active membership (by tier
//! order) and lasts until the billing period ends plus `grace_days`. It is
//! stored as the "whop" entitlement, which a manual tier set by an admin
//! still overrides.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::{
    AppState,
    accounts::AccountError,
    config::WhopConfig,
    repo::{
        entitlements::SOURCE_WHOP,
        whop::WhopMembership,
    },
    services::{check, http_client},
    tiers,
};

/// Membership states that grant access. Whop keeps retrying a failed
/// renewal while `past_due`, and sends `membership.deactivated` if it gives up.
const ENTITLED_STATUSES: &[&str] = &["active", "trialing", "past_due", "canceling"];
/// Webhooks older or newer than this are rejected (replay protection).
const SIGNATURE_TOLERANCE_SECS: i64 = 5 * 60;

pub fn is_entitled(status: &str) -> bool {
    ENTITLED_STATUSES.contains(&status)
}

// ---------------------------------------------------------------------------
// Webhook signatures
// ---------------------------------------------------------------------------

/// Checks a Standard Webhooks signature as Whop sends it. Whop signs
/// `{webhook-id}.{webhook-timestamp}.{body}` with HMAC-SHA256 keyed with the
/// literal bytes of the `ws_…` secret (not base64-decoded, unlike the plain
/// Standard Webhooks scheme), and the `webhook-signature` header holds one or
/// more space-separated `v1,<base64>` values.
pub fn verify_signature(
    secret: &str,
    id: &str,
    timestamp: &str,
    signature_header: &str,
    body: &[u8],
    now_unix: i64,
) -> Result<(), String> {
    let sent_at: i64 = timestamp
        .trim()
        .parse()
        .map_err(|_| "webhook-timestamp is not a number".to_owned())?;
    if (now_unix - sent_at).abs() > SIGNATURE_TOLERANCE_SECS {
        return Err("webhook-timestamp is outside the allowed window".to_owned());
    }

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| "invalid webhook secret".to_owned())?;
    mac.update(id.as_bytes());
    mac.update(b".");
    mac.update(timestamp.trim().as_bytes());
    mac.update(b".");
    mac.update(body);

    let matches = signature_header
        .split_whitespace()
        .filter_map(|entry| entry.strip_prefix("v1,"))
        .filter_map(|encoded| BASE64.decode(encoded).ok())
        .any(|signature| mac.clone().verify_slice(&signature).is_ok());
    if matches {
        Ok(())
    } else {
        Err("no matching webhook signature".to_owned())
    }
}

// ---------------------------------------------------------------------------
// Membership data
// ---------------------------------------------------------------------------

/// The fields we use from a Whop membership. Accepts both shapes Whop's API
/// documents (`plan_id` vs `plan.id`, `current_period_end` vs
/// `renewal_period_end`, `user_id` vs `user.id`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MembershipInfo {
    pub id: String,
    pub plan_id: String,
    pub status: String,
    pub whop_user_id: Option<String>,
    pub email: Option<String>,
    /// Our user id, set as checkout metadata by the upgrade button.
    pub metadata_user_id: Option<String>,
    /// The membership an upgrade checkout replaces (checkout metadata); it is
    /// cancelled once this one is active.
    pub replaces_membership: Option<String>,
    pub current_period_start: Option<String>,
    pub current_period_end: Option<String>,
    pub cancel_at_period_end: bool,
    pub manage_url: Option<String>,
}

impl MembershipInfo {
    pub fn from_json(value: &Value) -> Option<Self> {
        let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        Some(Self {
            id: text(&value["id"])?,
            plan_id: text(&value["plan_id"]).or_else(|| text(&value["plan"]["id"]))?,
            status: text(&value["status"])?,
            whop_user_id: text(&value["user_id"]).or_else(|| text(&value["user"]["id"])),
            email: text(&value["user"]["email"]).or_else(|| text(&value["email"])),
            metadata_user_id: text(&value["metadata"]["user_id"]),
            replaces_membership: text(&value["metadata"]["replaces_membership"]),
            current_period_start: text(&value["current_period_start"])
                .or_else(|| text(&value["renewal_period_start"])),
            current_period_end: text(&value["current_period_end"])
                .or_else(|| text(&value["renewal_period_end"])),
            cancel_at_period_end: value["cancel_at_period_end"].as_bool().unwrap_or(false),
            manage_url: text(&value["manage_url"]),
        })
    }
}

// ---------------------------------------------------------------------------
// API client
// ---------------------------------------------------------------------------

/// A plan's price in Whop, in dollars (or the plan's currency).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanPrice {
    /// One-time amount added to the first payment.
    pub initial: f64,
    /// Charged every billing period, including the first.
    pub renewal: f64,
}

impl PlanPrice {
    pub fn first_charge_cents(&self) -> i64 {
        to_cents(self.initial + self.renewal)
    }

    pub fn renewal_cents(&self) -> i64 {
        to_cents(self.renewal)
    }
}

pub fn to_cents(amount: f64) -> i64 {
    (amount * 100.0).round() as i64
}

const PLAN_PRICE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[derive(Clone, Debug)]
pub struct WhopClient {
    http: reqwest::Client,
    base: String,
    api_key: String,
    account_id: String,
    plan_prices: std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, PlanPrice)>>,
    >,
}

impl WhopClient {
    pub fn new(config: &WhopConfig) -> Self {
        Self {
            http: http_client(),
            base: config.api_url.trim_end_matches('/').to_owned(),
            api_key: config.api_key.clone(),
            account_id: config.account_id.clone(),
            plan_prices: Default::default(),
        }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.api_key)
    }

    /// Creates a checkout for `plan_id` and returns its purchase URL. The
    /// metadata is copied onto the membership, which is how its webhooks are
    /// matched back to the account.
    pub async fn create_checkout(
        &self,
        plan_id: &str,
        metadata: &Value,
        redirect_url: &str,
    ) -> Result<String, String> {
        let response = self
            .request(reqwest::Method::POST, "/checkout_configurations")
            .json(&json!({
                "account_id": self.account_id,
                "plan_id": plan_id,
                "redirect_url": redirect_url,
                "metadata": metadata,
            }))
            .send()
            .await
            .map_err(|error| format!("creating Whop checkout failed: {error}"))?;
        let body: Value = check(response, "creating Whop checkout")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Whop checkout failed: {error}"))?;
        body["purchase_url"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Whop checkout has no purchase_url".to_owned())
    }

    /// The plan's price, cached for a few minutes.
    pub async fn plan_price(&self, plan_id: &str) -> Result<PlanPrice, String> {
        if let Some((fetched, price)) = self.plan_prices.lock().expect("cache lock").get(plan_id)
            && fetched.elapsed() < PLAN_PRICE_TTL
        {
            return Ok(*price);
        }
        let response = self
            .request(reqwest::Method::GET, &format!("/plans/{plan_id}"))
            .send()
            .await
            .map_err(|error| format!("loading Whop plan failed: {error}"))?;
        let plan: Value = check(response, "loading Whop plan")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Whop plan failed: {error}"))?;
        let price = PlanPrice {
            initial: plan["initial_price"].as_f64().unwrap_or(0.0),
            renewal: plan["renewal_price"].as_f64().unwrap_or(0.0),
        };
        self.plan_prices
            .lock()
            .expect("cache lock")
            .insert(plan_id.to_owned(), (std::time::Instant::now(), price));
        Ok(price)
    }

    /// Creates a single-use discount for the first payment of `plan_id`.
    /// Returns the code as Whop stores it (lowercased).
    pub async fn create_promo_code(
        &self,
        code: &str,
        amount_cents: i64,
        plan_id: &str,
        expires_at: &str,
    ) -> Result<String, String> {
        let response = self
            .request(reqwest::Method::POST, "/promo_codes")
            .json(&json!({
                "account_id": self.account_id,
                "code": code,
                // Whop's docs say "amount"; the API only accepts "flat_amount".
                "promo_type": "flat_amount",
                "amount_off": amount_cents as f64 / 100.0,
                "base_currency": "usd",
                "plan_ids": [plan_id],
                "stock": 1,
                "one_per_customer": true,
                "new_users_only": false,
                "promo_duration_months": 1,
                "expires_at": expires_at,
            }))
            .send()
            .await
            .map_err(|error| format!("creating Whop promo code failed: {error}"))?;
        let body: Value = check(response, "creating Whop promo code")
            .await?
            .json()
            .await
            .map_err(|error| format!("reading Whop promo code failed: {error}"))?;
        Ok(body["code"].as_str().unwrap_or(code).to_owned())
    }

    /// Cancels a membership: at the end of its period (it stops renewing) or
    /// immediately.
    pub async fn cancel_membership(
        &self,
        membership_id: &str,
        at_period_end: bool,
        reason: &str,
    ) -> Result<(), String> {
        let response = self
            .request(reqwest::Method::POST, &format!("/memberships/{membership_id}/cancel"))
            .json(&json!({ "cancel_at_period_end": at_period_end, "reason": reason }))
            .send()
            .await
            .map_err(|error| format!("cancelling Whop membership failed: {error}"))?;
        check(response, "cancelling Whop membership").await?;
        Ok(())
    }

    /// Undoes a scheduled cancellation, so the membership renews again.
    pub async fn resume_membership(&self, membership_id: &str) -> Result<(), String> {
        let response = self
            .request(reqwest::Method::PATCH, &format!("/memberships/{membership_id}"))
            .json(&json!({ "cancel_at_period_end": false }))
            .send()
            .await
            .map_err(|error| format!("resuming Whop membership failed: {error}"))?;
        check(response, "resuming Whop membership").await?;
        Ok(())
    }

    /// Every membership of the account, following pagination.
    pub async fn list_memberships(&self) -> Result<Vec<MembershipInfo>, String> {
        let mut memberships = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut query = vec![("account_id", self.account_id.clone()), ("first", "100".to_owned())];
            if let Some(after) = &cursor {
                query.push(("after", after.clone()));
            }
            let response = self
                .request(reqwest::Method::GET, "/memberships")
                .query(&query)
                .send()
                .await
                .map_err(|error| format!("listing Whop memberships failed: {error}"))?;
            let page: Value = check(response, "listing Whop memberships")
                .await?
                .json()
                .await
                .map_err(|error| format!("reading Whop memberships failed: {error}"))?;
            memberships.extend(
                page["data"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(MembershipInfo::from_json),
            );
            cursor = page["page_info"]["end_cursor"].as_str().map(str::to_owned);
            if page["page_info"]["has_next_page"].as_bool() != Some(true) || cursor.is_none() {
                return Ok(memberships);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Applying memberships
// ---------------------------------------------------------------------------

/// Records a membership, links it to an account if possible, and updates
/// that account's paid tier. Returns the matched user id, or `None` if the
/// membership couldn't be matched (it then waits on /admin/payments).
pub async fn apply_membership(
    state: &AppState,
    info: &MembershipInfo,
) -> Result<Option<String>, String> {
    let user_id = match_user(state, info).await?;
    state
        .whop
        .upsert(&WhopMembership {
            membership_id: info.id.clone(),
            user_id: user_id.clone(),
            whop_user_id: info.whop_user_id.clone(),
            email: info.email.clone(),
            plan_id: info.plan_id.clone(),
            status: info.status.clone(),
            current_period_start: info.current_period_start.clone(),
            current_period_end: info.current_period_end.clone(),
            cancel_at_period_end: info.cancel_at_period_end,
            manage_url: info.manage_url.clone(),
            updated_at: String::new(),
        })
        .await
        .map_err(|error| format!("storing Whop membership failed: {error}"))?;

    // A stored membership may already be linked (e.g. by an admin).
    let linked = match user_id {
        Some(user_id) => Some(user_id),
        None => state
            .whop
            .find(&info.id)
            .await
            .map_err(|error| format!("loading Whop membership failed: {error}"))?
            .and_then(|membership| membership.user_id),
    };
    if let Some(user_id) = &linked {
        if is_entitled(&info.status) {
            crate::plan_change::on_membership_active(state, user_id, info).await;
        }
        refresh_entitlement(state, user_id).await?;
    }
    Ok(linked)
}

/// Finds the account a membership belongs to: the user id from checkout
/// metadata, then an existing link, then an LLDAP account with the same email.
async fn match_user(state: &AppState, info: &MembershipInfo) -> Result<Option<String>, String> {
    if let Some(user_id) = &info.metadata_user_id {
        let user = state
            .users
            .find_by_id(user_id)
            .await
            .map_err(|error| format!("loading user failed: {error}"))?;
        if user.is_some() {
            return Ok(Some(user_id.clone()));
        }
    }
    let known = state
        .whop
        .find(&info.id)
        .await
        .map_err(|error| format!("loading Whop membership failed: {error}"))?
        .and_then(|membership| membership.user_id);
    if known.is_some() {
        return Ok(known);
    }
    if let Some(email) = &info.email {
        let username = state
            .lldap
            .username_for_email(email)
            .await
            .map_err(|error| format!("LLDAP email lookup failed: {error}"))?;
        if let Some(username) = username {
            let user = state
                .users
                .ensure(&username)
                .await
                .map_err(|error| format!("user mirror failed: {error}"))?;
            return Ok(Some(user.id));
        }
    }
    Ok(None)
}

/// Sets the user's "whop" entitlement from their best active membership, or
/// removes it, then queues a sync of every service.
pub async fn refresh_entitlement(state: &AppState, user_id: &str) -> Result<(), String> {
    let memberships = state
        .whop
        .for_user(user_id)
        .await
        .map_err(|error| format!("loading Whop memberships failed: {error}"))?;
    let config = &state.config;
    let best = memberships
        .iter()
        .filter(|membership| is_entitled(&membership.status))
        .filter_map(|membership| {
            config
                .tier_for_whop_plan(&membership.plan_id)
                .map(|tier| (membership, tier))
        })
        .max_by_key(|(_, tier)| config.tier_rank(&tier.id));

    match best {
        Some((membership, tier)) => {
            let valid_until = membership
                .current_period_end
                .as_deref()
                .and_then(|end| DateTime::parse_from_rfc3339(end).ok())
                .map(|end| {
                    (end.with_timezone(&Utc) + Duration::days(config.whop.grace_days)).to_rfc3339()
                });
            let note = format!("Whop membership {}", membership.membership_id);
            state
                .entitlements
                .set(user_id, SOURCE_WHOP, &tier.id, valid_until.as_deref(), Some(&note), Some("whop"))
                .await
                .map_err(|error| format!("saving paid tier failed: {error}"))?;
            state
                .audit
                .record_best_effort(
                    Some("whop"),
                    "whop_tier_set",
                    Some(user_id),
                    Some(json!({ "tier": tier.id, "membership": membership.membership_id, "valid_until": valid_until })),
                )
                .await;
        }
        None => {
            let removed = state
                .entitlements
                .clear(user_id, SOURCE_WHOP)
                .await
                .map_err(|error| format!("clearing paid tier failed: {error}"))?;
            if removed {
                state
                    .audit
                    .record_best_effort(Some("whop"), "whop_tier_cleared", Some(user_id), None)
                    .await;
            }
        }
    }
    tiers::queue_sync(state, user_id)
        .await
        .map_err(|error| error.to_string())
}

/// Links an unmatched membership to an account (from /admin/payments) and
/// applies it.
pub async fn assign_membership(
    state: &AppState,
    membership_id: &str,
    username: &str,
    actor: &str,
) -> Result<(), AccountError> {
    let internal = |context: &str, error: &dyn std::fmt::Display| {
        AccountError::Internal(format!("{context}: {error}"))
    };
    let username = username.trim().to_ascii_lowercase();
    if state
        .lldap
        .get_user(&username)
        .await
        .map_err(|error| internal("LLDAP lookup failed", &error))?
        .is_none()
    {
        return Err(AccountError::Rejected(format!("There is no account named \"{username}\".")));
    }
    let user = state
        .users
        .ensure(&username)
        .await
        .map_err(|error| internal("user mirror failed", &error))?;
    if !state
        .whop
        .assign(membership_id, &user.id)
        .await
        .map_err(|error| internal("linking membership failed", &error))?
    {
        return Err(AccountError::Rejected("That membership no longer exists.".to_owned()));
    }
    state
        .audit
        .record_best_effort(Some(actor), "whop_membership_assigned", Some(&username), Some(json!({ "membership": membership_id })))
        .await;
    refresh_entitlement(state, &user.id)
        .await
        .map_err(AccountError::Internal)
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

/// An HTTP status and short body for the webhook endpoint.
pub struct WebhookReply {
    pub status: u16,
    pub body: &'static str,
}

impl WebhookReply {
    const fn new(status: u16, body: &'static str) -> Self {
        Self { status, body }
    }
}

/// Handles POST /api/webhooks/whop. Only answers 2xx once the event is
/// recorded; anything else makes Whop retry.
pub async fn handle_webhook(
    state: &AppState,
    id: Option<&str>,
    timestamp: Option<&str>,
    signature: Option<&str>,
    body: &[u8],
) -> WebhookReply {
    if !state.config.whop.enabled {
        return WebhookReply::new(404, "whop is not enabled");
    }
    let (Some(id), Some(timestamp), Some(signature)) = (id, timestamp, signature) else {
        return WebhookReply::new(400, "missing webhook headers");
    };
    if let Err(error) = verify_signature(
        &state.config.whop.webhook_secret,
        id,
        timestamp,
        signature,
        body,
        Utc::now().timestamp(),
    ) {
        log::warn!("rejected Whop webhook {id}: {error}");
        return WebhookReply::new(401, "invalid signature");
    }
    let Ok(event) = serde_json::from_slice::<Value>(body) else {
        return WebhookReply::new(400, "body is not JSON");
    };
    let event_type = event["type"].as_str().unwrap_or("unknown").to_owned();

    match state.whop.claim_event(id, &event_type).await {
        Ok(true) => {}
        Ok(false) => return WebhookReply::new(200, "duplicate"),
        Err(error) => {
            log::error!("recording Whop webhook {id} failed: {error}");
            return WebhookReply::new(500, "storage error");
        }
    }

    let (outcome, membership_id, detail) = if event_type.starts_with("membership.") {
        match MembershipInfo::from_json(&event["data"]) {
            None => ("error", None, Some("membership data is incomplete".to_owned())),
            Some(info) => match apply_membership(state, &info).await {
                Ok(Some(_)) => ("applied", Some(info.id), None),
                Ok(None) => ("unmatched", Some(info.id), Some("no account matches this membership".to_owned())),
                Err(error) => {
                    // Let Whop's retry try again later.
                    log::error!("applying Whop webhook {id} failed: {error}");
                    let _ = state.whop.release_event(id).await;
                    return WebhookReply::new(500, "processing error");
                }
            },
        }
    } else {
        ("ignored", None, None)
    };
    if let Err(error) = state
        .whop
        .finish_event(id, membership_id.as_deref(), outcome, detail.as_deref())
        .await
    {
        log::error!("recording Whop webhook {id} outcome failed: {error}");
    }
    log::info!("Whop webhook {id} ({event_type}): {outcome}");
    WebhookReply::new(200, "ok")
}

/// Re-applies every membership Whop reports, to repair missed webhooks.
pub async fn reconcile(state: &AppState) -> Result<usize, String> {
    let Some(client) = &state.whop_client else {
        return Ok(0);
    };
    let memberships = client.list_memberships().await?;
    let mut applied = 0;
    for info in memberships
        .iter()
        .filter(|info| state.config.tier_for_whop_plan(&info.plan_id).is_some())
    {
        apply_membership(state, info).await?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    use serde_json::json;
    use sha2::Sha256;

    use super::{MembershipInfo, verify_signature};

    const SECRET: &str = "ws_test_secret";

    fn sign(id: &str, timestamp: i64, body: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(format!("{id}.{timestamp}.{body}").as_bytes());
        format!("v1,{}", super::BASE64.encode(mac.finalize().into_bytes()))
    }

    #[test]
    fn accepts_a_valid_signature_among_several() {
        let body = r#"{"type":"membership.activated"}"#;
        let now = 1_800_000_000;
        let header = format!("v1,bm9wZQ== {}", sign("msg_1", now, body));
        assert!(verify_signature(SECRET, "msg_1", &now.to_string(), &header, body.as_bytes(), now).is_ok());
    }

    #[test]
    fn rejects_tampering_wrong_secrets_and_stale_timestamps() {
        let body = r#"{"type":"membership.activated"}"#;
        let now = 1_800_000_000;
        let header = sign("msg_1", now, body);
        let ts = now.to_string();

        assert!(verify_signature(SECRET, "msg_1", &ts, &header, br#"{"type":"x"}"#, now).is_err());
        assert!(verify_signature(SECRET, "msg_2", &ts, &header, body.as_bytes(), now).is_err());
        assert!(verify_signature("ws_other", "msg_1", &ts, &header, body.as_bytes(), now).is_err());
        assert!(verify_signature(SECRET, "msg_1", &ts, &header, body.as_bytes(), now + 301).is_err());
        assert!(verify_signature(SECRET, "msg_1", "soon", &header, body.as_bytes(), now).is_err());
        assert!(verify_signature(SECRET, "msg_1", &ts, "v2,abc", body.as_bytes(), now).is_err());
    }

    #[test]
    fn signs_with_the_literal_secret_bytes() {
        // The plain Standard Webhooks scheme would base64-decode the secret;
        // Whop doesn't. A signature made with the decoded bytes must fail.
        let body = "{}";
        let now = 1_800_000_000;
        let decoded_key = super::BASE64.decode("dGVzdA==").unwrap();
        let mut mac = Hmac::<Sha256>::new_from_slice(&decoded_key).unwrap();
        mac.update(format!("msg_1.{now}.{body}").as_bytes());
        let header = format!("v1,{}", super::BASE64.encode(mac.finalize().into_bytes()));
        assert!(verify_signature("dGVzdA==", "msg_1", &now.to_string(), &header, body.as_bytes(), now).is_err());
    }

    #[test]
    fn parses_both_documented_membership_shapes() {
        let current = json!({
            "id": "mem_1", "plan_id": "plan_plus", "status": "active", "user_id": "user_1",
            "current_period_end": "2026-12-01T00:00:00.000Z",
            "manage_url": "https://whop.com/billing/manage/mem_1",
            "metadata": { "user_id": "our-user" }
        });
        let info = MembershipInfo::from_json(&current).unwrap();
        assert_eq!(info.plan_id, "plan_plus");
        assert_eq!(info.whop_user_id.as_deref(), Some("user_1"));
        assert_eq!(info.metadata_user_id.as_deref(), Some("our-user"));
        assert_eq!(info.current_period_end.as_deref(), Some("2026-12-01T00:00:00.000Z"));

        let older = json!({
            "id": "mem_2", "plan": { "id": "plan_plus" }, "status": "canceled",
            "user": { "id": "user_2", "email": "bob@example.com" },
            "renewal_period_end": "2026-11-01T00:00:00.000Z", "metadata": {}
        });
        let info = MembershipInfo::from_json(&older).unwrap();
        assert_eq!(info.plan_id, "plan_plus");
        assert_eq!(info.email.as_deref(), Some("bob@example.com"));
        assert_eq!(info.metadata_user_id, None);
        assert_eq!(info.current_period_end.as_deref(), Some("2026-11-01T00:00:00.000Z"));

        assert!(MembershipInfo::from_json(&json!({ "id": "mem_3" })).is_none());
    }

    mod client {
        use serde_json::json;
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{body_partial_json, header, method, path, query_param},
        };

        use crate::{config::WhopConfig, whop::WhopClient};

        fn client(server: &MockServer) -> WhopClient {
            WhopClient::new(&WhopConfig {
                enabled: true,
                api_url: server.uri(),
                api_key: "key".to_owned(),
                account_id: "biz_1".to_owned(),
                webhook_secret: "ws_x".to_owned(),
                grace_days: 3,
            })
        }

        #[tokio::test]
        async fn creates_a_checkout_carrying_the_user_id() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/checkout_configurations"))
                .and(header("Authorization", "Bearer key"))
                .and(body_partial_json(json!({
                    "account_id": "biz_1",
                    "plan_id": "plan_plus",
                    "redirect_url": "https://acct/billing?checkout=done",
                    "metadata": { "user_id": "u1", "username": "alice" }
                })))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "id": "ch_1", "purchase_url": "https://whop.com/checkout/ch_1/"
                })))
                .expect(1)
                .mount(&server)
                .await;

            let url = client(&server)
                .create_checkout(
                    "plan_plus",
                    &json!({ "user_id": "u1", "username": "alice" }),
                    "https://acct/billing?checkout=done",
                )
                .await
                .unwrap();
            assert_eq!(url, "https://whop.com/checkout/ch_1/");
        }

        #[tokio::test]
        async fn lists_memberships_across_pages() {
            let server = MockServer::start().await;
            let membership = |id: &str| json!({ "id": id, "plan_id": "plan_plus", "status": "active" });
            Mock::given(method("GET"))
                .and(path("/memberships"))
                .and(query_param("account_id", "biz_1"))
                .and(query_param("after", "cursor-1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": [membership("mem_3")],
                    "page_info": { "has_next_page": false, "end_cursor": null }
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/memberships"))
                .and(query_param("account_id", "biz_1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": [membership("mem_1"), membership("mem_2"), { "id": "incomplete" }],
                    "page_info": { "has_next_page": true, "end_cursor": "cursor-1" }
                })))
                .mount(&server)
                .await;

            let ids: Vec<String> = client(&server)
                .list_memberships()
                .await
                .unwrap()
                .into_iter()
                .map(|membership| membership.id)
                .collect();
            assert_eq!(ids, ["mem_1", "mem_2", "mem_3"]);
        }
    }

    mod applying {
        use crate::{
            AppState,
            config::{AppConfig, TierConfig},
            repo::entitlements::SOURCE_WHOP,
            whop::{MembershipInfo, apply_membership},
        };

        fn tier(id: &str, plans: &[&str]) -> TierConfig {
            TierConfig {
                id: id.to_owned(),
                name: id.to_owned(),
                description: String::new(),
                price_label: None,
                lldap_groups: Vec::new(),
                whop_plan_ids: plans.iter().map(|plan| (*plan).to_owned()).collect(),
                jellyfin: None,
                seerr: None,
            }
        }

        async fn state() -> (AppState, String, std::path::PathBuf) {
            let path = std::env::temp_dir().join(format!("am-whop-{}.db", uuid::Uuid::new_v4()));
            let config = AppConfig {
                database: crate::config::DatabaseConfig {
                    url: format!("sqlite://{}", path.display()),
                },
                tiers: vec![
                    tier("free", &[]),
                    tier("plus", &["plan_plus", "plan_plus_yearly"]),
                    tier("premium", &["plan_premium"]),
                ],
                ..AppConfig::default()
            };
            let state = AppState::initialize(config).await.expect("state should initialize");
            let user = state.users.ensure("alice").await.unwrap();
            (state, user.id, path)
        }

        fn membership(id: &str, plan: &str, status: &str, user_id: Option<&str>) -> MembershipInfo {
            MembershipInfo {
                id: id.to_owned(),
                plan_id: plan.to_owned(),
                status: status.to_owned(),
                whop_user_id: Some("user_1".to_owned()),
                email: None,
                metadata_user_id: user_id.map(str::to_owned),
                replaces_membership: None,
                current_period_start: Some("2026-11-01T00:00:00.000Z".to_owned()),
                current_period_end: Some("2026-12-01T00:00:00.000Z".to_owned()),
                cancel_at_period_end: false,
                manage_url: None,
            }
        }

        async fn whop_tier(state: &AppState, user_id: &str) -> Option<(String, Option<String>)> {
            state
                .entitlements
                .for_user(user_id)
                .await
                .unwrap()
                .into_iter()
                .find(|e| e.source == SOURCE_WHOP)
                .map(|e| (e.tier, e.valid_until))
        }

        #[tokio::test]
        async fn memberships_set_and_clear_the_paid_tier() {
            let (state, user_id, path) = state().await;

            let matched = apply_membership(&state, &membership("mem_1", "plan_plus", "active", Some(&user_id)))
                .await
                .unwrap();
            assert_eq!(matched.as_deref(), Some(user_id.as_str()));
            let (tier, until) = whop_tier(&state, &user_id).await.unwrap();
            assert_eq!(tier, "plus");
            // Period end plus the default 3 grace days.
            assert_eq!(until.as_deref(), Some("2026-12-04T00:00:00+00:00"));
            assert!(!state.provisioning.jobs().await.unwrap().is_empty(), "a sync is queued");

            // A second, higher membership wins; cancelling it falls back.
            apply_membership(&state, &membership("mem_2", "plan_premium", "active", Some(&user_id)))
                .await
                .unwrap();
            assert_eq!(whop_tier(&state, &user_id).await.unwrap().0, "premium");
            apply_membership(&state, &membership("mem_2", "plan_premium", "canceled", None))
                .await
                .unwrap();
            assert_eq!(whop_tier(&state, &user_id).await.unwrap().0, "plus");

            apply_membership(&state, &membership("mem_1", "plan_plus", "expired", None))
                .await
                .unwrap();
            assert_eq!(whop_tier(&state, &user_id).await, None);

            let _ = std::fs::remove_file(path);
        }

        #[tokio::test]
        async fn unmatched_memberships_wait_for_an_admin() {
            let (state, user_id, path) = state().await;

            let matched = apply_membership(&state, &membership("mem_9", "plan_plus", "active", None))
                .await
                .unwrap();
            assert_eq!(matched, None);
            assert_eq!(state.whop.unmatched().await.unwrap().len(), 1);
            assert_eq!(whop_tier(&state, &user_id).await, None);

            // Metadata naming an account that doesn't exist doesn't match either.
            let matched = apply_membership(&state, &membership("mem_8", "plan_plus", "active", Some("ghost")))
                .await
                .unwrap();
            assert_eq!(matched, None);

            let _ = std::fs::remove_file(path);
        }
    }
}
