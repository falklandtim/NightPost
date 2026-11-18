//! The store's signatures: the named, shared signatures, which one each
//! account uses by default, and the one-off move from the old
//! one-signature-per-account column.

use super::*;

impl Store {
    /// Older databases kept one signature string per account; turn those
    /// into shared, named signatures (once — the old column is then cleared).
    pub(super) fn migrate_signatures(&self) -> Result<()> {
        let legacy: Vec<(AccountId, String)> = {
            let mut st = self.conn.prepare("SELECT id, signature FROM accounts WHERE signature <> ''")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        for (id, body) in legacy {
            let sig = self.signature_for_text(&body)?;
            self.set_default_signature(id, Some(sig))?;
            self.conn.execute("UPDATE accounts SET signature='' WHERE id=?1", [id])?;
        }
        Ok(())
    }

    /// An existing signature with this exact text, or a new one.
    pub(super) fn signature_for_text(&self, body: &str) -> Result<i64> {
        if let Some(id) = self.conn.query_row("SELECT id FROM signatures WHERE body=?1", [body], |r| r.get(0)).optional()? {
            return Ok(id);
        }
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM signatures", [], |r| r.get(0))?;
        let title = if n == 0 { "Signature".to_string() } else { format!("Signature {}", n + 1) };
        self.save_signature(&Signature { id: 0, title, body: body.to_string(), html: false })
    }


    /// Every saved signature, oldest first.
    pub fn signatures(&self) -> Result<Vec<Signature>> {
        let mut st = self.conn.prepare("SELECT id, title, body, html FROM signatures ORDER BY id")?;
        let v = st
            .query_map([], |r| Ok(Signature { id: r.get(0)?, title: r.get(1)?, body: r.get(2)?, html: r.get(3)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// The signature an account uses by default.
    pub fn default_signature(&self, account_id: AccountId) -> Result<Option<Signature>> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT signature_id FROM account_signatures WHERE account_id=?1", [account_id], |r| r.get(0))
            .optional()?;
        Ok(id.and_then(|id| self.signatures().ok()?.into_iter().find(|s| s.id == id)))
    }

    /// Insert (id 0) or update a signature; returns its id.
    pub fn save_signature(&self, s: &Signature) -> Result<i64> {
        if s.id == 0 {
            self.conn.execute("INSERT INTO signatures (title, body, html) VALUES (?1, ?2, ?3)", params![s.title, s.body, s.html])?;
            Ok(self.conn.last_insert_rowid())
        } else {
            self.conn.execute("UPDATE signatures SET title=?2, body=?3, html=?4 WHERE id=?1", params![s.id, s.title, s.body, s.html])?;
            Ok(s.id)
        }
    }

    /// Delete a signature; accounts using it as their default lose it.
    pub fn delete_signature(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM signatures WHERE id=?1", [id])?;
        Ok(())
    }

    /// Account → default signature id.
    pub fn signature_defaults(&self) -> Result<HashMap<AccountId, i64>> {
        let mut st = self.conn.prepare("SELECT account_id, signature_id FROM account_signatures")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Set (or, with `None`, clear) an account's default signature.
    pub fn set_default_signature(&self, account_id: AccountId, signature: Option<i64>) -> Result<()> {
        match signature {
            Some(sig) => self.conn.execute(
                "INSERT INTO account_signatures (account_id, signature_id) VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET signature_id=excluded.signature_id",
                params![account_id, sig],
            )?,
            None => self.conn.execute("DELETE FROM account_signatures WHERE account_id=?1", [account_id])?,
        };
        Ok(())
    }
}
