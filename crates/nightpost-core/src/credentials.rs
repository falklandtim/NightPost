//! Passwords live in the OS keychain (macOS Keychain, Windows Credential
//! Manager, Secret Service on Linux) — never in the SQLite database.

use anyhow::{Context, Result};

const SERVICE: &str = "NightPost";
/// Keychain services from before the renames, newest first. Secrets are
/// copied forward from them on first use. OwlPost's are left in place, since
/// an older OwlPost may still run on this machine (or be the MCP server);
/// SpringPost's are removed once copied, as they always were.
const LEGACY_SERVICES: [&str; 2] = ["OwlPost", "SpringPost"];
/// The service whose entries are deleted after being carried forward.
const RETIRED_SERVICE: &str = "SpringPost";

pub fn set_password(email: &str, password: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, email)?
        .set_password(password)
        .context("saving password to the system keychain")
}

pub fn get_password(email: &str) -> Result<String> {
    // Debug builds only (local testing): in the app people use, anything
    // that can set an environment variable could otherwise swap in a
    // password. Nothing NightPost runs sets it (asked 2026-11-17).
    #[cfg(debug_assertions)]
    if let Ok(p) = std::env::var(format!("NIGHTPOST_PASSWORD_{}", env_key(email))) {
        return Ok(p);
    }
    get_carried(email).with_context(|| format!("no saved password for {email} in the system keychain"))
}

pub fn delete_password(email: &str) -> Result<()> {
    forget_password(email);
    delete_refresh_token(email);
    Ok(())
}

/// Remove just the password (an account that switched to Google sign-in).
pub fn forget_password(email: &str) {
    for service in std::iter::once(SERVICE).chain(LEGACY_SERVICES) {
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

/// Any other secret NightPost keeps (e.g. the Google OAuth client).
pub fn set_secret(key: &str, value: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, key)?.set_password(value).context("saving to the system keychain")
}

pub fn get_secret(key: &str) -> Result<String> {
    get_carried(key).with_context(|| format!("nothing saved for {key} in the system keychain"))
}

/// Delete a secret, including any copy left under an old name, so a
/// deleted sign-in isn't carried forward again on the next read.
pub fn delete_secret(key: &str) -> Result<()> {
    for service in std::iter::once(SERVICE).chain(LEGACY_SERVICES) {
        let _ = keyring::Entry::new(service, key)?.delete_credential();
    }
    Ok(())
}

/// Read `key` from NightPost's service, or else from an old one; a value
/// found under an old name is saved under NightPost's for next time.
fn get_carried(key: &str) -> Result<String> {
    if let Ok(v) = keyring::Entry::new(SERVICE, key)?.get_password() {
        return Ok(v);
    }
    for service in LEGACY_SERVICES {
        let Ok(old) = keyring::Entry::new(service, key) else { continue };
        let Ok(v) = old.get_password() else { continue };
        if keyring::Entry::new(SERVICE, key)?.set_password(&v).is_ok() && service == RETIRED_SERVICE {
            let _ = old.delete_credential();
        }
        return Ok(v);
    }
    anyhow::bail!("not in the system keychain")
}

/// `tim@icloud.com` → `TIM_ICLOUD_COM`, for headless/CI overrides.
#[cfg(debug_assertions)]
fn env_key(email: &str) -> String {
    email
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}
