//! Passwords live in the OS keychain (macOS Keychain, Windows Credential
//! Manager, Secret Service on Linux) — never in the SQLite database.

use anyhow::{Context, Result};

const SERVICE: &str = "OwlPost";
/// Passwords saved before the app was renamed from SpringPost.
const LEGACY_SERVICE: &str = "SpringPost";

pub fn set_password(email: &str, password: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, email)?
        .set_password(password)
        .context("saving password to the system keychain")
}

pub fn get_password(email: &str) -> Result<String> {
    if let Ok(p) = std::env::var(format!("OWLPOST_PASSWORD_{}", env_key(email))) {
        return Ok(p);
    }
    if let Ok(p) = keyring::Entry::new(SERVICE, email)?.get_password() {
        return Ok(p);
    }
    // Carry a SpringPost-era password over to the OwlPost keychain entry.
    let p = keyring::Entry::new(LEGACY_SERVICE, email)?
        .get_password()
        .with_context(|| format!("no saved password for {email} in the system keychain"))?;
    if set_password(email, &p).is_ok() {
        let _ = keyring::Entry::new(LEGACY_SERVICE, email)?.delete_credential();
    }
    Ok(p)
}

pub fn delete_password(email: &str) -> Result<()> {
    let _ = keyring::Entry::new(SERVICE, email)?.delete_credential();
    let _ = keyring::Entry::new(LEGACY_SERVICE, email)?.delete_credential();
    delete_refresh_token(email);
    Ok(())
}

/// Remove just the password (an account that switched to Google sign-in).
pub fn forget_password(email: &str) {
    for service in [SERVICE, LEGACY_SERVICE] {
        if let Ok(e) = keyring::Entry::new(service, email) {
            let _ = e.delete_credential();
        }
    }
}

/// A Google sign-in's refresh token, kept apart from any password.
pub fn set_refresh_token(email: &str, token: &str) -> Result<()> {
    set_secret(&format!("google-oauth:{}", email.to_ascii_lowercase()), token)
}

pub fn get_refresh_token(email: &str) -> Result<String> {
    get_secret(&format!("google-oauth:{}", email.to_ascii_lowercase()))
}

pub fn delete_refresh_token(email: &str) {
    let _ = delete_secret(&format!("google-oauth:{}", email.to_ascii_lowercase()));
}

/// Any other secret OwlPost keeps (e.g. the Google OAuth client).
pub fn set_secret(key: &str, value: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, key)?.set_password(value).context("saving to the system keychain")
}

pub fn get_secret(key: &str) -> Result<String> {
    keyring::Entry::new(SERVICE, key)?.get_password().with_context(|| format!("nothing saved for {key} in the system keychain"))
}

pub fn delete_secret(key: &str) -> Result<()> {
    let _ = keyring::Entry::new(SERVICE, key)?.delete_credential();
    Ok(())
}

/// `tim@icloud.com` → `TIM_ICLOUD_COM`, for headless/CI overrides.
fn env_key(email: &str) -> String {
    email
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}
