//! Background worker that applies each user's tier to every service.
//!
//! Jobs live in `sync_jobs`, so nothing is lost on restart and requests never
//! wait on Jellyfin or Seerr. Failed jobs retry with exponential backoff. A
//! periodic reconcile queues jobs for accounts that are missing or whose
//! applied settings no longer match: a different tier (e.g. a manual tier
//! expired) or a changed tier configuration.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    accounts::now_rfc3339,
    config::{AppConfig, TierConfig},
    repo::{
        entitlements::Entitlement,
        provisioning::{
            PRIORITY_BACKGROUND, PRIORITY_NOW, STATE_OK, STATE_SKIPPED, ServiceAccount, SyncJob,
        },
    },
    services::{Service, SyncOutcome},
    tiers::{self, EffectiveTier},
};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(10 * 60);
const WHOP_RECONCILE_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const BATCH_SIZE: i64 = 50;

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        let mut last_reconcile: Option<tokio::time::Instant> = None;
        let mut last_whop_reconcile: Option<tokio::time::Instant> = None;
        loop {
            poll.tick().await;
            // Daily (and at startup): re-apply every Whop membership, in case
            // a webhook was missed.
            if state.whop_client.is_some()
                && last_whop_reconcile.is_none_or(|at| at.elapsed() >= WHOP_RECONCILE_INTERVAL)
            {
                match crate::whop::reconcile(&state).await {
                    Ok(count) => log::info!("Whop reconcile applied {count} memberships"),
                    Err(error) => log::error!("Whop reconcile failed: {error}"),
                }
                last_whop_reconcile = Some(tokio::time::Instant::now());
            }
            if last_reconcile.is_none_or(|at| at.elapsed() >= RECONCILE_INTERVAL) {
                match reconcile(&state).await {
                    Ok(0) => {}
                    Ok(queued) => log::info!("reconcile queued {queued} sync jobs"),
                    Err(error) => log::error!("reconcile failed: {error}"),
                }
                last_reconcile = Some(tokio::time::Instant::now());
            }
            if let Err(error) = run_due(&state).await {
                log::error!("sync worker failed: {error}");
            }
        }
    });
}

/// Delay before attempt `attempts + 1`: 30s, 1m, 2m, … capped at 1h.
pub fn backoff(attempts: i64) -> Duration {
    let exponent = attempts.clamp(0, 7) as u32;
    Duration::from_secs((30u64 << exponent).min(60 * 60))
}

/// Runs every job that is due. Returns how many ran.
pub async fn run_due(state: &AppState) -> Result<usize, String> {
    let mut jobs = state
        .provisioning
        .due_jobs(&now_rfc3339(), BATCH_SIZE)
        .await
        .map_err(|error| format!("loading due jobs failed: {error}"))?;
    // Seerr imports from Jellyfin, so within a batch run services in order.
    jobs.sort_by_key(|job| {
        Service::parse(&job.service).map_or(usize::MAX, |service| {
            Service::ALL.iter().position(|s| *s == service).unwrap_or(usize::MAX)
        })
    });
    for job in &jobs {
        run_job(state, job).await;
    }
    Ok(jobs.len())
}

async fn run_job(state: &AppState, job: &SyncJob) {
    let result = sync_one(state, job).await;
    let recorded = match result {
        Ok((outcome, applied_tier)) => {
            let status = if outcome.skipped { STATE_SKIPPED } else { STATE_OK };
            state
                .provisioning
                .complete(
                    &job.user_id,
                    &job.service,
                    status,
                    outcome.remote_id.as_deref(),
                    applied_tier.as_deref(),
                    outcome.detail.as_deref(),
                )
                .await
        }
        Err(error) => {
            let delay = backoff(job.attempts);
            log::warn!(
                "sync of {} for user {} failed (attempt {}), retrying in {}s: {error}",
                job.service,
                job.user_id,
                job.attempts + 1,
                delay.as_secs()
            );
            let next = Utc::now()
                + chrono::Duration::from_std(delay).expect("backoff fits in chrono::Duration");
            state
                .provisioning
                .fail(&job.user_id, &job.service, &error, &next.to_rfc3339())
                .await
        }
    };
    if let Err(error) = recorded {
        log::error!("recording sync result for user {} failed: {error}", job.user_id);
    }
}

/// Applies the user's effective tier to one service. Returns the outcome and
/// the tier that was applied (`None` if nothing was applied).
async fn sync_one(
    state: &AppState,
    job: &SyncJob,
) -> Result<(SyncOutcome, Option<String>), String> {
    let service = Service::parse(&job.service).ok_or_else(|| format!("unknown service {}", job.service))?;
    let user = state
        .users
        .find_by_id(&job.user_id)
        .await
        .map_err(|error| format!("loading user failed: {error}"))?
        .ok_or("user no longer exists")?;
    if user.disabled_at.is_some() {
        return Ok((SyncOutcome::skipped(None, "account disabled"), None));
    }
    let effective = tiers::effective_tier(state, &user.id)
        .await
        .map_err(|error| error.to_string())?;
    let tier = &effective.tier;

    let outcome = match service {
        Service::Lldap => sync_groups(state, &user.username, &effective).await?,
        Service::Jellyfin => {
            let client = state.services.jellyfin.as_ref().ok_or("jellyfin is disabled")?;
            client.sync(&user.username, tier.jellyfin.as_ref()).await?
        }
        Service::Seerr => {
            let client = state.services.seerr.as_ref().ok_or("seerr is disabled")?;
            let jellyfin = state
                .provisioning
                .account(&user.id, Service::Jellyfin.as_str())
                .await
                .map_err(|error| format!("loading Jellyfin account failed: {error}"))?;
            let jellyfin_id = jellyfin
                .filter(|account| account.state == STATE_OK || account.state == STATE_SKIPPED)
                .and_then(|account| account.remote_id)
                .ok_or("waiting for the Jellyfin account")?;
            client.sync(&user.username, &jellyfin_id, tier.seerr.as_ref()).await?
        }
    };
    Ok((outcome, Some(settings_key(&state.config, tier, service))))
}

/// Puts the user in their tier's groups and takes them out of groups that
/// belong only to other tiers. Groups no tier mentions are left alone.
async fn sync_groups(
    state: &AppState,
    username: &str,
    effective: &EffectiveTier,
) -> Result<SyncOutcome, String> {
    let Some(profile) = state
        .lldap
        .get_user(username)
        .await
        .map_err(|error| format!("LLDAP lookup failed: {error}"))?
    else {
        return Ok(SyncOutcome::skipped(None, "not in LLDAP"));
    };
    let current: HashSet<&str> = profile.groups.iter().map(|g| g.display_name.as_str()).collect();
    let wanted = &effective.tier.lldap_groups;
    let add: Vec<String> = wanted
        .iter()
        .filter(|group| !current.contains(group.as_str()))
        .cloned()
        .collect();
    let remove: Vec<String> = state
        .config
        .tier_managed_groups()
        .into_iter()
        .filter(|group| current.contains(group.as_str()) && !wanted.contains(group))
        .collect();
    state
        .lldap
        .update_groups(username, &add, &remove)
        .await
        .map_err(|error| format!("updating LLDAP groups failed: {error}"))?;
    Ok(SyncOutcome::done(None))
}

/// Identifies what a sync of `service` applies for `tier`: the tier id plus a
/// short hash of the settings involved. Stored as the account's applied
/// tier, so editing a tier's settings (not only switching tiers) marks its
/// accounts out of date and the reconcile re-applies them.
pub fn settings_key(config: &AppConfig, tier: &TierConfig, service: Service) -> String {
    let settings = match service {
        Service::Lldap => format!("{:?}|{:?}", tier.lldap_groups, config.tier_managed_groups()),
        Service::Jellyfin => format!(
            "{:?}|{:?}",
            tier.jellyfin, config.jellyfin.login_attempts_before_lockout
        ),
        Service::Seerr => format!("{:?}", tier.seerr),
    };
    let digest = format!("{:x}", Sha256::digest(settings.as_bytes()));
    format!("{}#{}", tier.id, &digest[..12])
}

/// Which services need a sync for this user: no account record yet, or one
/// whose applied settings differ from what `tier` would apply now. Services
/// with a queued job are left to that job.
pub fn needs_sync(
    config: &AppConfig,
    tier: &TierConfig,
    enabled: &[Service],
    accounts: &[&ServiceAccount],
    queued: &HashSet<&str>,
) -> Vec<Service> {
    enabled
        .iter()
        .copied()
        .filter(|service| !queued.contains(service.as_str()))
        .filter(|service| {
            match accounts.iter().find(|account| account.service == service.as_str()) {
                None => true,
                Some(account) => {
                    (account.state == STATE_OK || account.state == STATE_SKIPPED)
                        && account.applied_tier.as_deref()
                            != Some(settings_key(config, tier, *service).as_str())
                }
            }
        })
        .collect()
}

/// Queues jobs for one user's missing or out-of-date accounts. Called on
/// login and signup so new and pre-existing users get set up promptly.
pub async fn reconcile_user(state: &AppState, user_id: &str) -> Result<usize, String> {
    let effective = tiers::effective_tier(state, user_id)
        .await
        .map_err(|error| error.to_string())?;
    let accounts = state
        .provisioning
        .accounts_for_user(user_id)
        .await
        .map_err(|error| error.to_string())?;
    let jobs = state.provisioning.jobs().await.map_err(|error| error.to_string())?;
    let queued: HashSet<&str> = jobs
        .iter()
        .filter(|job| job.user_id == user_id)
        .map(|job| job.service.as_str())
        .collect();
    let accounts: Vec<&ServiceAccount> = accounts.iter().collect();
    let due = needs_sync(
        &state.config,
        &effective.tier,
        &state.services.enabled(),
        &accounts,
        &queued,
    );
    let jobs: Vec<(String, String)> = due
        .iter()
        .map(|service| (user_id.to_owned(), service.as_str().to_owned()))
        .collect();
    state
        .provisioning
        .enqueue_many(&jobs, &now_rfc3339(), PRIORITY_NOW)
        .await
        .map_err(|error| error.to_string())?;
    Ok(jobs.len())
}

/// Queues jobs for every user whose accounts are missing or out of date.
pub async fn reconcile(state: &AppState) -> Result<usize, String> {
    let users = state.users.all().await.map_err(|error| error.to_string())?;
    let entitlements = state.entitlements.all().await.map_err(|error| error.to_string())?;
    let accounts = state.provisioning.all_accounts().await.map_err(|error| error.to_string())?;
    let jobs = state.provisioning.jobs().await.map_err(|error| error.to_string())?;

    let mut entitlements_by_user: HashMap<&str, Vec<Entitlement>> = HashMap::new();
    for entitlement in &entitlements {
        entitlements_by_user
            .entry(entitlement.user_id.as_str())
            .or_default()
            .push(entitlement.clone());
    }
    let enabled = state.services.enabled();
    let now = now_rfc3339();
    let mut due: Vec<(String, String)> = Vec::new();
    for user in users.iter().filter(|user| user.disabled_at.is_none()) {
        let effective = tiers::resolve(
            &state.config,
            entitlements_by_user.get(user.id.as_str()).map(Vec::as_slice).unwrap_or_default(),
            &now,
        );
        let user_accounts: Vec<&ServiceAccount> =
            accounts.iter().filter(|account| account.user_id == user.id).collect();
        let user_jobs: HashSet<&str> = jobs
            .iter()
            .filter(|job| job.user_id == user.id)
            .map(|job| job.service.as_str())
            .collect();
        for service in needs_sync(&state.config, &effective.tier, &enabled, &user_accounts, &user_jobs) {
            due.push((user.id.clone(), service.as_str().to_owned()));
        }
    }
    // One transaction for the whole pass.
    state
        .provisioning
        .enqueue_many(&due, &now, PRIORITY_BACKGROUND)
        .await
        .map_err(|error| error.to_string())?;
    Ok(due.len())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, time::Duration};

    use super::{backoff, needs_sync, settings_key};
    use crate::{
        config::{AppConfig, JellyfinTier},
        repo::provisioning::ServiceAccount,
        services::Service,
    };

    fn account(service: &str, state: &str, applied: Option<&str>) -> ServiceAccount {
        ServiceAccount {
            user_id: "u".to_owned(),
            service: service.to_owned(),
            remote_id: None,
            applied_tier: applied.map(str::to_owned),
            state: state.to_owned(),
            detail: None,
            updated_at: String::new(),
        }
    }

    #[test]
    fn backoff_doubles_up_to_an_hour() {
        assert_eq!(backoff(0), Duration::from_secs(30));
        assert_eq!(backoff(1), Duration::from_secs(60));
        assert_eq!(backoff(3), Duration::from_secs(240));
        assert_eq!(backoff(20), Duration::from_secs(3600));
    }

    #[test]
    fn syncs_missing_and_out_of_date_accounts_only() {
        let config = AppConfig::default();
        let tier = config.tiers[0].clone();
        let enabled = [Service::Lldap, Service::Jellyfin, Service::Seerr];
        let current = settings_key(&config, &tier, Service::Lldap);
        let lldap = account("lldap", "ok", Some(&current));
        let jellyfin = account("jellyfin", "skipped", Some("other-tier#000000000000"));
        let none_queued = HashSet::new();

        // lldap is current, jellyfin is behind, seerr has no record yet.
        let due = needs_sync(&config, &tier, &enabled, &[&lldap, &jellyfin], &none_queued);
        assert_eq!(due, vec![Service::Jellyfin, Service::Seerr]);

        // Already-queued services are left to their job.
        let queued: HashSet<&str> = ["seerr"].into();
        let due = needs_sync(&config, &tier, &enabled, &[&lldap, &jellyfin], &queued);
        assert_eq!(due, vec![Service::Jellyfin]);

        // Pending and errored accounts are handled by their job's retries.
        let pending = account("lldap", "pending", None);
        assert!(needs_sync(&config, &tier, &[Service::Lldap], &[&pending], &none_queued).is_empty());
    }

    #[test]
    fn editing_a_tier_or_shared_setting_changes_its_key() {
        let mut config = AppConfig::default();
        let mut tier = config.tiers[0].clone();
        let before = settings_key(&config, &tier, Service::Jellyfin);
        assert!(before.starts_with("free#"));
        assert_eq!(before, settings_key(&config, &tier, Service::Jellyfin), "stable");

        tier.jellyfin = Some(JellyfinTier { live_tv: Some(false), ..JellyfinTier::default() });
        let edited = settings_key(&config, &tier, Service::Jellyfin);
        assert_ne!(before, edited);

        config.jellyfin.login_attempts_before_lockout = Some(5);
        assert_ne!(edited, settings_key(&config, &tier, Service::Jellyfin));
        // Seerr's key ignores Jellyfin settings.
        assert_eq!(
            settings_key(&AppConfig::default(), &tier, Service::Seerr),
            settings_key(&config, &tier, Service::Seerr)
        );
    }
}
