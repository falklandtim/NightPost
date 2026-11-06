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
    Ok(())
}

/// `tim@icloud.com` → `TIM_ICLOUD_COM`, for headless/CI overrides.
fn env_key(email: &str) -> String {
    email
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}
