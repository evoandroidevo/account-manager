use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct PasswordPolicy {
    pub minimum_length: usize,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_number: bool,
    pub require_special: bool,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            minimum_length: 12,
            require_uppercase: true,
            require_lowercase: true,
            require_number: true,
            require_special: true,
        }
    }
}

/// User-safe: at least 3 characters, starting with a letter, using only
/// letters, numbers, periods, hyphens, and underscores.
pub fn validate_username(username: &str) -> Result<(), String> {
    let username = username.trim();

    if username.len() < 3 || username.len() > 32 {
        return Err("Username must be between 3 and 32 characters.".to_owned());
    }
    if !username.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Err("Username must start with a letter.".to_owned());
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(
            "Username may only contain letters, numbers, periods, hyphens, and underscores."
                .to_owned(),
        );
    }

    Ok(())
}

/// User-safe: a single `@`, a non-empty local part, and a domain containing a dot.
pub fn validate_email(email: &str) -> Result<(), String> {
    let email = email.trim();

    if email.is_empty() || email.chars().any(char::is_whitespace) {
        return Err("Enter a valid email address.".to_owned());
    }
    if email.matches('@').count() != 1 {
        return Err("Enter a valid email address.".to_owned());
    }

    let Some((local, domain)) = email.split_once('@') else {
        return Err("Enter a valid email address.".to_owned());
    };

    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err("Enter a valid email address.".to_owned());
    }

    Ok(())
}

/// User-safe: checks the password against the configured policy.
pub fn validate_password(password: &str, policy: &PasswordPolicy) -> Result<(), String> {
    if password.len() < policy.minimum_length {
        return Err(format!(
            "Password must be at least {} characters.",
            policy.minimum_length
        ));
    }
    if policy.require_uppercase && !password.chars().any(|c| c.is_uppercase()) {
        return Err("Password must contain an uppercase letter.".to_owned());
    }
    if policy.require_lowercase && !password.chars().any(|c| c.is_lowercase()) {
        return Err("Password must contain a lowercase letter.".to_owned());
    }
    if policy.require_number && !password.chars().any(|c| c.is_numeric()) {
        return Err("Password must contain a number.".to_owned());
    }
    if policy.require_special && !password.chars().any(|c| !c.is_alphanumeric()) {
        return Err("Password must contain a symbol.".to_owned());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PasswordPolicy, validate_email, validate_password, validate_username};

    #[test]
    fn accepts_a_well_formed_username() {
        assert!(validate_username("bob_smith-1").is_ok());
    }

    #[test]
    fn rejects_a_username_that_is_too_short() {
        assert!(validate_username("ab").is_err());
    }

    #[test]
    fn rejects_a_username_that_does_not_start_with_a_letter() {
        assert!(validate_username("1bob").is_err());
    }

    #[test]
    fn rejects_a_username_with_disallowed_characters() {
        assert!(validate_username("bob smith@").is_err());
    }

    #[test]
    fn accepts_a_well_formed_email() {
        assert!(validate_email("new.user@example.com").is_ok());
    }

    #[test]
    fn rejects_an_email_without_an_at_sign() {
        assert!(validate_email("user-example.com").is_err());
    }

    #[test]
    fn rejects_an_email_with_more_than_one_at_sign() {
        assert!(validate_email("user@sub@example.com").is_err());
    }

    #[test]
    fn rejects_an_email_with_no_dot_in_the_domain() {
        assert!(validate_email("user@localhost").is_err());
    }

    #[test]
    fn accepts_a_strong_password_under_the_default_policy() {
        let policy = PasswordPolicy::default();
        assert!(validate_password("Str0ng!Passw0rd", &policy).is_ok());
    }

    #[test]
    fn rejects_a_password_shorter_than_the_configured_minimum() {
        let policy = PasswordPolicy::default();
        assert!(validate_password("Sh0rt!", &policy).is_err());
    }

    #[test]
    fn rejects_a_password_missing_a_required_character_class() {
        let policy = PasswordPolicy::default();
        assert!(validate_password("alllowercase123!", &policy).is_err());
    }

    #[test]
    fn accepts_a_simple_password_when_the_policy_relaxes_requirements() {
        let policy = PasswordPolicy {
            minimum_length: 4,
            require_uppercase: false,
            require_lowercase: false,
            require_number: false,
            require_special: false,
        };
        assert!(validate_password("pass", &policy).is_ok());
    }
}
