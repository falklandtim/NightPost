//! "Sign in with Google": OAuth 2.0 for Gmail and Google Workspace.
//!
//! The browser flow for installed apps (RFC 8252): OwlPost listens on a
//! random local port, opens Google's sign-in page in the browser, and Google
//! sends the browser back to `http://127.0.0.1:<port>` with a one-time code.
//! PKCE (RFC 7636) ties the code to this sign-in. The code is exchanged for a
//! refresh token (kept in the keychain) and short-lived access tokens, which
//! sign in to IMAP and SMTP with XOAUTH2. OwlPost never sees the password.
//!
//! Each install uses its own OAuth client from Google Cloud (see the README).

use crate::credentials;
use crate::models::{Account, Auth};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Full mail access (IMAP/SMTP needs it), plus who you are.
pub const SCOPES: &str = "https://mail.google.com/ openid email";
/// How long to wait for the browser to come back.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// An OAuth client ("Desktop app") from Google Cloud. For installed apps
/// Google doesn't treat the secret as confidential; it's kept in the
/// keychain anyway.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoogleClient {
    pub id: String,
    pub secret: String,
}

impl GoogleClient {
    /// The client saved in Preferences, if any.
    pub fn load() -> Option<Self> {
        let raw = credentials::get_secret("google-oauth-client").ok()?;
        let (id, secret) = raw.split_once('\n')?;
        (!id.trim().is_empty()).then(|| Self { id: id.trim().into(), secret: secret.trim().into() })
    }

    pub fn save(&self) -> Result<()> {
        if !self.id.trim().ends_with(".apps.googleusercontent.com") {
            bail!("that doesn't look like a Google client ID (it ends in .apps.googleusercontent.com)");
        }
        credentials::set_secret("google-oauth-client", &format!("{}\n{}", self.id.trim(), self.secret.trim()))
    }

    pub fn clear() {
        let _ = credentials::delete_secret("google-oauth-client");
    }
}

/// The result of signing in.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub email: String,
    pub name: String,
    pub refresh_token: String,
    pub access_token: String,
    pub expires_in: Duration,
}

/// A sign-in waiting for the browser.
pub struct Pending {
    listener: TcpListener,
    verifier: String,
    state: String,
    redirect: String,
    /// Open this in the browser.
    pub url: String,
}

fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("no randomness: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// The PKCE S256 challenge for a verifier.
pub fn challenge(verifier: &str) -> String {
    use sha2::Digest;
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

fn enc(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Start signing in: listen locally and build Google's sign-in URL.
/// `login_hint` pre-fills the account (when re-signing in).
pub fn begin(client: &GoogleClient, login_hint: Option<&str>) -> Result<Pending> {
    let listener = TcpListener::bind("127.0.0.1:0").context("listening for the browser")?;
    let redirect = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = random_token(48)?;
    let state = random_token(16)?;
    let mut url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}&access_type=offline&prompt=consent",
        enc(&client.id),
        enc(&redirect),
        enc(SCOPES),
        challenge(&verifier),
        state
    );
    if let Some(h) = login_hint {
        url.push_str(&format!("&login_hint={}", enc(h)));
    }
    Ok(Pending { listener, verifier, state, redirect, url })
}

impl Pending {
    /// Wait for the browser to return, then exchange the code for tokens.
    /// Gives up after `timeout`, or when `cancel` is set.
    pub fn finish(self, client: &GoogleClient, cancel: &AtomicBool, timeout: Duration) -> Result<SignedIn> {
        let code = self.wait_for_code(cancel, timeout)?;
        let v = post_form(&[
            ("code", code.as_str()),
            ("client_id", &client.id),
            ("client_secret", &client.secret),
            ("redirect_uri", &self.redirect),
            ("grant_type", "authorization_code"),
            ("code_verifier", &self.verifier),
        ])?;
        let refresh_token = v["refresh_token"].as_str().ok_or_else(|| anyhow!("Google didn't send a refresh token"))?.to_string();
        let access_token = v["access_token"].as_str().ok_or_else(|| anyhow!("Google didn't send an access token"))?.to_string();
        let expires_in = Duration::from_secs(v["expires_in"].as_u64().unwrap_or(3600));
        let claims = id_token_claims(v["id_token"].as_str().unwrap_or_default())?;
        let email = claims["email"].as_str().ok_or_else(|| anyhow!("Google didn't say which account you signed in with"))?.to_string();
        if claims["email_verified"].as_bool() == Some(false) {
            bail!("{email} isn't verified with Google");
        }
        let name = claims["name"].as_str().unwrap_or_default().to_string();
        Ok(SignedIn { email, name, refresh_token, access_token, expires_in })
    }

    /// Accept browser requests until one carries the code (or an error).
    fn wait_for_code(&self, cancel: &AtomicBool, timeout: Duration) -> Result<String> {
        self.listener.set_nonblocking(true)?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("sign-in cancelled");
            }
            if Instant::now() > deadline {
                bail!("timed out waiting for the browser — try again");
            }
            let mut stream = match self.listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let Some(params) = read_request(&stream) else { continue };
            // Other requests (e.g. /favicon.ico) get a plain 404.
            if !params.contains_key("code") && !params.contains_key("error") {
                respond(&mut stream, "404 Not Found", "");
                continue;
            }
            if params.get("state").map(String::as_str) != Some(self.state.as_str()) {
                respond(&mut stream, "400 Bad Request", &page("Something went wrong", "This sign-in link didn't match. Go back to OwlPost and try again."));
                bail!("the browser returned an unexpected sign-in (state mismatch)");
            }
            if let Some(err) = params.get("error") {
                respond(&mut stream, "200 OK", &page("Sign-in cancelled", "You can close this tab and return to OwlPost."));
                bail!(if err == "access_denied" { "sign-in was cancelled in the browser".to_string() } else { format!("Google refused the sign-in: {err}") });
            }
            respond(&mut stream, "200 OK", &page("You're signed in", "You can close this tab and return to OwlPost."));
            return Ok(params["code"].clone());
        }
    }
}

/// Parse `GET /?a=b&c=d HTTP/1.1` into its query parameters.
fn read_request(stream: &TcpStream) -> Option<HashMap<String, String>> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1)?;
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    Some(form_urlencoded::parse(query.as_bytes()).into_owned().collect())
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn page(title: &str, text: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>OwlPost</title>\
         <body style=\"font-family:-apple-system,system-ui,sans-serif;display:grid;place-items:center;height:90vh;margin:0;color:#222\">\
         <div style=\"text-align:center\"><h1 style=\"font-weight:600\">{title}</h1><p style=\"color:#666\">{text}</p></div>"
    )
}

/// The claims in an ID token. It came straight from Google's token
/// endpoint over TLS, so the signature needn't be checked (OIDC 3.1.3.7).
pub fn id_token_claims(id_token: &str) -> Result<serde_json::Value> {
    let payload = id_token.split('.').nth(1).ok_or_else(|| anyhow!("Google sent no ID token"))?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).context("reading the ID token")?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn post_form(form: &[(&str, &str)]) -> Result<serde_json::Value> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    match agent.post(TOKEN_URL).send_form(form) {
        Ok(r) => Ok(serde_json::from_str(&r.into_string()?)?),
        Err(ureq::Error::Status(code, r)) => {
            let body: serde_json::Value = serde_json::from_str(&r.into_string().unwrap_or_default()).unwrap_or_default();
            let err = body["error"].as_str().unwrap_or("error");
            if err == "invalid_grant" {
                bail!(SignInExpired);
            }
            let desc = body["error_description"].as_str().unwrap_or("");
            bail!("Google said {code} {err}{}{desc}", if desc.is_empty() { "" } else { ": " })
        }
        Err(e) => Err(anyhow!("couldn't reach Google: {e}")),
    }
}

/// The refresh token was revoked or expired: the user must sign in again.
#[derive(Debug)]
pub struct SignInExpired;
impl std::fmt::Display for SignInExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Google sign-in has expired or was revoked — sign in again in Preferences > Accounts")
    }
}
impl std::error::Error for SignInExpired {}

/// Access tokens in memory, by account email, with when they expire.
static TOKENS: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

fn remember(email: &str, token: &str, expires_in: Duration) {
    // Renew a few minutes early so a token never expires mid-connection.
    let until = Instant::now() + expires_in.saturating_sub(Duration::from_secs(300));
    TOKENS.lock().unwrap().get_or_insert_with(HashMap::new).insert(email.to_ascii_lowercase(), (token.to_string(), until));
}

/// Save a completed sign-in for an account's email.
pub fn store_sign_in(s: &SignedIn) -> Result<()> {
    credentials::set_refresh_token(&s.email, &s.refresh_token)?;
    remember(&s.email, &s.access_token, s.expires_in);
    Ok(())
}

/// A valid access token for a Google account: cached, or refreshed.
pub fn access_token(account: &Account) -> Result<String> {
    let key = account.email.to_ascii_lowercase();
    if let Some((t, until)) = TOKENS.lock().unwrap().as_ref().and_then(|m| m.get(&key).cloned()) {
        if Instant::now() < until {
            return Ok(t);
        }
    }
    let client = GoogleClient::load().ok_or_else(|| anyhow!("Google sign-in isn't set up — add your client ID in Preferences > Accounts"))?;
    let refresh = credentials::get_refresh_token(&account.email).map_err(|_| anyhow!(SignInExpired))?;
    let v = post_form(&[("client_id", &client.id), ("client_secret", &client.secret), ("refresh_token", &refresh), ("grant_type", "refresh_token")])?;
    let token = v["access_token"].as_str().ok_or_else(|| anyhow!("Google didn't send an access token"))?.to_string();
    remember(&account.email, &token, Duration::from_secs(v["expires_in"].as_u64().unwrap_or(3600)));
    Ok(token)
}

/// Forget a cached access token (after the server rejected it).
pub fn forget(account: &Account) {
    if let Some(m) = TOKENS.lock().unwrap().as_mut() {
        m.remove(&account.email.to_ascii_lowercase());
    }
}

/// The XOAUTH2 initial response: `user=…^Aauth=Bearer …^A^A`.
pub fn xoauth2(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

/// IMAP `AUTHENTICATE XOAUTH2`.
pub struct XOAuth2(pub String);

impl imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&self, challenge: &[u8]) -> String {
        // On failure Gmail sends a JSON challenge; an empty reply ends the
        // exchange so the server answers NO instead of hanging.
        if challenge.is_empty() {
            self.0.clone()
        } else {
            String::new()
        }
    }
}

/// Log in an IMAP client by whichever method the account uses.
pub fn imap_login<T: std::io::Read + std::io::Write>(client: imap::Client<T>, account: &Account) -> Result<imap::Session<T>> {
    match account.auth {
        Auth::Password => {
            let password = credentials::get_password(&account.email)?;
            client.login(&account.username, password).map_err(|(e, _)| anyhow!("login failed: {e}"))
        }
        Auth::Google => {
            let token = access_token(account)?;
            client.authenticate("XOAUTH2", &XOAuth2(xoauth2(&account.username, &token))).map_err(|(e, _)| {
                forget(account);
                anyhow!("Google sign-in was refused: {e}")
            })
        }
    }
}

/// Base64 of the XOAUTH2 string, as sent on the wire (for tests and logs).
pub fn xoauth2_base64(user: &str, token: &str) -> String {
    STANDARD.encode(xoauth2(user, token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_base64url_sha256() {
        // SHA-256("abc") = ba7816bf…15ad (FIPS 180-2), base64url without padding;
        // cross-checked with `openssl dgst -sha256 -binary | base64`.
        assert_eq!(challenge("abc"), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
        let v = random_token(48).unwrap();
        assert_eq!(v.len(), 64, "a 64-character verifier (RFC 7636 allows 43–128)");
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn xoauth2_string_is_what_gmail_expects() {
        // From Google's XOAUTH2 documentation.
        assert_eq!(
            xoauth2_base64("someuser@example.com", "ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg"),
            "dXNlcj1zb21ldXNlckBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciB5YTI5LnZGOWRmdDRxbVRjMk52YjNSbGNrQmhkSFJoZG1semRHRXVZMjl0Q2cBAQ=="
        );
        let a = XOAuth2("x".into());
        assert_eq!(imap::Authenticator::process(&a, b""), "x");
        assert_eq!(imap::Authenticator::process(&a, b"{\"status\":\"400\"}"), "");
    }

    #[test]
    fn reads_id_token_claims() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"tim@example.com","email_verified":true,"name":"Tim"}"#);
        let c = id_token_claims(&format!("hdr.{payload}.sig")).unwrap();
        assert_eq!(c["email"], "tim@example.com");
        assert!(id_token_claims("garbage").is_err());
    }

    #[test]
    fn browser_redirect_delivers_the_code() {
        let client = GoogleClient { id: "123.apps.googleusercontent.com".into(), secret: "s".into() };
        let p = begin(&client, Some("tim@example.com")).unwrap();
        assert!(p.url.starts_with(AUTH_URL));
        for part in ["code_challenge_method=S256", "access_type=offline", "login_hint=tim%40example.com", "scope=https%3A%2F%2Fmail.google.com%2F+openid+email"] {
            assert!(p.url.contains(part), "{part} in {}", p.url);
        }
        let port: u16 = p.redirect.rsplit(':').next().unwrap().parse().unwrap();
        let state = p.state.clone();
        // Play the browser: a favicon request, then the real redirect.
        let browser = std::thread::spawn(move || {
            let get = |path: &str| {
                let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
                let mut out = String::new();
                let _ = std::io::Read::read_to_string(&mut s, &mut out);
                out
            };
            let fav = get("/favicon.ico");
            let ok = get(&format!("/?state={state}&code=4%2F0Abc&scope=email"));
            (fav, ok)
        });
        let code = p.wait_for_code(&AtomicBool::new(false), Duration::from_secs(10)).unwrap();
        assert_eq!(code, "4/0Abc");
        let (fav, ok) = browser.join().unwrap();
        assert!(fav.starts_with("HTTP/1.1 404"));
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("You're signed in"));
    }

    #[test]
    fn wrong_state_or_cancel_is_refused() {
        let client = GoogleClient { id: "1.apps.googleusercontent.com".into(), secret: String::new() };
        let p = begin(&client, None).unwrap();
        let port: u16 = p.redirect.rsplit(':').next().unwrap().parse().unwrap();
        let t = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET /?state=forged&code=x HTTP/1.1\r\n\r\n").unwrap();
        });
        assert!(p.wait_for_code(&AtomicBool::new(false), Duration::from_secs(10)).unwrap_err().to_string().contains("state mismatch"));
        t.join().unwrap();
        let p = begin(&client, None).unwrap();
        assert!(p.wait_for_code(&AtomicBool::new(true), Duration::from_secs(10)).unwrap_err().to_string().contains("cancelled"));
        let p = begin(&client, None).unwrap();
        assert!(p.wait_for_code(&AtomicBool::new(false), Duration::from_millis(200)).unwrap_err().to_string().contains("timed out"));
    }

    #[test]
    fn client_ids_are_checked() {
        assert!(GoogleClient { id: "not-a-client".into(), secret: "x".into() }.save().is_err());
    }
}
