//! "Sign in with Google" on the iPhone.
//!
//! The phone can't run the desktop's local web server, so the sign-in is
//! split in two around Apple's sign-in sheet (`ASWebAuthenticationSession`):
//!
//! 1. [`Mail::start_google_sign_in`] builds Google's sign-in link for the
//!    app's iOS OAuth client, returning to the app through the client ID
//!    reversed ([`oauth::ios_redirect`]).
//! 2. Swift opens [`GoogleSignIn::url`] in the sheet, which closes when
//!    Google sends the browser to [`GoogleSignIn::callback_scheme`].
//! 3. [`Mail::finish_google_sign_in`] checks that link belongs to this
//!    sign-in, redeems the code, and adds the account (or switches an
//!    existing one from an app password to Google).
//!
//! The client ID is built into the app (see docs/ios.md), and is saved in
//! the phone's keychain only once a sign-in works, because token refresh
//! ([`oauth::access_token`]) reads it from there.

use super::{account_info, fail, AccountInfo, Mail, Result};
use nightpost_core::oauth::{self, GoogleClient};
use nightpost_core::presets::Provider;
use nightpost_core::{credentials, engine, Account, Auth};
use std::sync::Arc;

/// A sign-in in progress, between opening Google's page and its answer.
#[derive(uniffi::Object)]
pub struct GoogleSignIn {
    auth: oauth::Authorize,
    client: GoogleClient,
}

#[uniffi::export]
impl GoogleSignIn {
    /// Google's sign-in page: open it in `ASWebAuthenticationSession`.
    pub fn url(&self) -> String {
        self.auth.url.clone()
    }

    /// The link scheme Google returns to (the client ID reversed): the
    /// sheet's `callbackURLScheme`.
    pub fn callback_scheme(&self) -> String {
        let redirect = oauth::ios_redirect(&self.client.id);
        redirect.split(':').next().unwrap_or_default().to_string()
    }
}

#[uniffi::export]
impl Mail {
    /// Start signing in to Google with the app's iOS OAuth `client_id`.
    /// `login_hint` pre-fills the address (when re-signing in an account).
    /// Doesn't touch the network or the keychain.
    pub fn start_google_sign_in(&self, client_id: String, login_hint: Option<String>) -> Result<Arc<GoogleSignIn>> {
        let id = client_id.trim();
        if !id.ends_with(".apps.googleusercontent.com") || id.len() <= ".apps.googleusercontent.com".len() {
            return Err(fail("that isn't a Google client ID (it ends in .apps.googleusercontent.com)"));
        }
        // iOS clients have no secret.
        let client = GoogleClient { id: id.to_string(), secret: String::new() };
        let auth = oauth::authorize(&client, &oauth::ios_redirect(id), login_hint.as_deref())?;
        Ok(Arc::new(GoogleSignIn { auth, client }))
    }

    /// Finish a sign-in with the link the sheet returned. Adds the Google
    /// account (or switches an account already here over to Google sign-in,
    /// forgetting its app password) and returns it; call [`Mail::sync`]
    /// next. Blocks on the network.
    pub fn finish_google_sign_in(&self, sign_in: Arc<GoogleSignIn>, redirect: String) -> Result<AccountInfo> {
        // Checked first: a link that isn't ours never reaches Google.
        let code = sign_in.auth.code_from_redirect(&redirect)?;
        let signed = sign_in.auth.exchange(&sign_in.client, &code)?;
        sign_in.client.save()?;
        oauth::store_sign_in(&signed)?;
        let worker = self.worker()?;
        if let Some(mut acc) = worker.account_by_email(&signed.email)? {
            if acc.auth == Auth::Password {
                acc.auth = Auth::Google;
                worker.update_account(&acc)?;
                // The app password isn't needed any more.
                credentials::forget_password(&acc.email);
            }
            return Ok(account_info(acc));
        }
        let p = Provider::Gmail;
        let (Some(imap), Some(smtp)) = (p.imap(), p.smtp()) else {
            return Err(fail("NightPost doesn't know Gmail's servers"));
        };
        let name = if signed.name.is_empty() { signed.email.split('@').next().unwrap_or_default().to_string() } else { signed.name.clone() };
        let mut acc = Account {
            id: 0,
            display_name: name,
            email: signed.email.clone(),
            username: signed.email.clone(),
            imap,
            smtp,
            color: p.color().into(),
            signature: String::new(),
            demo: false,
            auth: Auth::Google,
        };
        let mut session = engine::connect(&acc)
            .map_err(|e| fail(&format!("signed in, but Gmail refused the connection: is IMAP turned on for this account? ({e:#})")))?;
        let _ = session.logout();
        acc.id = worker.add_account(&acc)?;
        Ok(account_info(acc))
    }
}
