//! Address autocomplete, built from everyone you've exchanged mail with.

use super::*;

impl Store {
    /// Address-book style autocomplete built from everyone you've exchanged mail with.
    pub fn suggest_contacts(&self, prefix: &str, limit: usize) -> Result<Vec<Contact>> {
        let like = format!("%{}%", prefix.trim());
        // People who wrote to you, plus people you wrote to (weighted higher).
        let mut st = self.conn.prepare(
            "WITH mine AS (SELECT lower(email) e FROM accounts),
             people(name, email, w) AS (
                SELECT from_name, from_email, 1 FROM messages WHERE lower(from_email) NOT IN (SELECT e FROM mine)
                UNION ALL
                SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 3
                FROM messages m, json_each(m.to_json || '') j WHERE lower(m.from_email) IN (SELECT e FROM mine)
                UNION ALL
                SELECT json_extract(j.value, '$.name'), json_extract(j.value, '$.email'), 3
                FROM messages m, json_each(m.cc_json || '') j WHERE lower(m.from_email) IN (SELECT e FROM mine)
             )
             SELECT COALESCE(MAX(name), ''), email, SUM(w) c FROM people
             WHERE email LIKE '%@%' AND (email LIKE ?1 OR name LIKE ?1) AND lower(email) NOT IN (SELECT e FROM mine)
             GROUP BY lower(email) ORDER BY c DESC LIMIT ?2",
        )?;
        let v = st
            .query_map(params![like, limit as i64], |r| Ok(Contact { name: r.get(0)?, email: r.get(1)? }))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}
