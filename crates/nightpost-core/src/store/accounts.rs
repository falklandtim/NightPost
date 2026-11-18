//! The store's accounts: adding, changing, removing and listing them.
//! Passwords aren't here; they live in the system keychain
//! ([`crate::credentials`]).

use super::*;

impl Store {
    /// Save a new account (its `id` is ignored) and return its id. A
    /// signature on it becomes a named signature, set as its default.
    pub fn add_account(&self, a: &Account) -> Result<AccountId> {
        self.conn.execute(
            "INSERT INTO accounts (display_name,email,username,imap_host,imap_port,imap_security,
                smtp_host,smtp_port,smtp_security,color,signature,demo,auth)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                a.display_name, a.email, a.username,
                a.imap.host, a.imap.port, a.imap.security.as_str(),
                a.smtp.host, a.smtp.port, a.smtp.security.as_str(),
                a.color, "", a.demo, a.auth.as_str()
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        // A signature on a new account becomes (or reuses) a named signature.
        if !a.signature.trim().is_empty() {
            let sig = self.signature_for_text(&a.signature)?;
            self.set_default_signature(id, Some(sig))?;
        }
        Ok(id)
    }

    /// Save changes to an account's name, servers, colour and sign-in.
    /// The email address and signature are not changed here.
    pub fn update_account(&self, a: &Account) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET display_name=?2, username=?3, imap_host=?4, imap_port=?5,
                imap_security=?6, smtp_host=?7, smtp_port=?8, smtp_security=?9, color=?10, auth=?11
             WHERE id=?1",
            params![
                a.id, a.display_name, a.username,
                a.imap.host, a.imap.port, a.imap.security.as_str(),
                a.smtp.host, a.smtp.port, a.smtp.security.as_str(),
                a.color, a.auth.as_str()
            ],
        )?;
        Ok(())
    }

    /// Delete an account and, through the foreign keys, its folders, mail,
    /// snoozes and pins. Its keychain entry is the caller's to remove.
    pub fn remove_account(&self, id: AccountId) -> Result<()> {
        self.conn.execute("DELETE FROM accounts WHERE id=?1", [id])?;
        Ok(())
    }

    /// Every account, in the order they were added.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let mut st = self.conn.prepare(
            "SELECT id,display_name,email,username,imap_host,imap_port,imap_security,
                    smtp_host,smtp_port,smtp_security,color,
                    '', demo, auth
             FROM accounts ORDER BY id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                display_name: r.get(1)?,
                email: r.get(2)?,
                username: r.get(3)?,
                imap: ServerConfig {
                    host: r.get(4)?,
                    port: r.get(5)?,
                    security: Security::parse(&r.get::<_, String>(6)?),
                },
                smtp: ServerConfig {
                    host: r.get(7)?,
                    port: r.get(8)?,
                    security: Security::parse(&r.get::<_, String>(9)?),
                },
                color: r.get(10)?,
                signature: r.get(11)?,
                demo: r.get(12)?,
                auth: Auth::parse(&r.get::<_, String>(13)?),
            })
        })?;
        let mut accounts: Vec<Account> = rows.collect::<rusqlite::Result<_>>()?;
        // `signature` is the default signature as text (HTML ones rendered).
        let sigs = self.signatures()?;
        for (acc, sig) in self.signature_defaults()? {
            if let (Some(a), Some(s)) = (accounts.iter_mut().find(|a| a.id == acc), sigs.iter().find(|s| s.id == sig)) {
                a.signature = s.text();
            }
        }
        Ok(accounts)
    }

    /// One account by id.
    pub fn account(&self, id: AccountId) -> Result<Option<Account>> {
        Ok(self.accounts()?.into_iter().find(|a| a.id == id))
    }

    /// One account by address (ignoring case).
    pub fn account_by_email(&self, email: &str) -> Result<Option<Account>> {
        Ok(self
            .accounts()?
            .into_iter()
            .find(|a| a.email.eq_ignore_ascii_case(email)))
    }
}
