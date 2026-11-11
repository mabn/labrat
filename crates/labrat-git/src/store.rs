//! Object and ref storage in SQLite. Objects are content-addressed and shared by all
//! repositories; refs belong to one repository (a project id).

use crate::object::{Commit, Kind, Oid, Tag, TreeEntry, parse_tree};
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub const SCHEMA: &str = r#"
CREATE TABLE git_objects (
    oid BLOB PRIMARY KEY,
    kind INTEGER NOT NULL,
    data BLOB NOT NULL
) WITHOUT ROWID;

-- Latest timestamp labrat gave a commit or tag it created in a repository.
CREATE TABLE git_clock (
    repo_id INTEGER PRIMARY KEY,
    last_time INTEGER NOT NULL
);

CREATE TABLE git_refs (
    repo_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    oid BLOB NOT NULL,
    PRIMARY KEY (repo_id, name)
) WITHOUT ROWID;
"#;

/// A repository view over a connection.
#[derive(Clone, Copy)]
pub struct Repo<'a> {
    pub c: &'a Connection,
    pub id: i64,
}

impl<'a> Repo<'a> {
    pub fn new(c: &'a Connection, id: i64) -> Repo<'a> {
        Repo { c, id }
    }

    pub fn write(&self, kind: Kind, data: &[u8]) -> Result<Oid> {
        let oid = Oid::hash(kind, data);
        self.c.execute(
            "INSERT OR IGNORE INTO git_objects (oid, kind, data) VALUES (?1, ?2, ?3)",
            params![&oid.0[..], kind as i64, data],
        )?;
        Ok(oid)
    }

    pub fn read(&self, oid: &Oid) -> Result<(Kind, Vec<u8>)> {
        self.try_read(oid)?.ok_or(Error::NotFound(*oid))
    }

    pub fn try_read(&self, oid: &Oid) -> Result<Option<(Kind, Vec<u8>)>> {
        let row: Option<(i64, Vec<u8>)> = self
            .c
            .query_row(
                "SELECT kind, data FROM git_objects WHERE oid = ?1",
                [&oid.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((k, d)) => Ok(Some((Kind::from_code(k as u8)?, d))),
            None => Ok(None),
        }
    }

    pub fn exists(&self, oid: &Oid) -> Result<bool> {
        let n: Option<i64> = self
            .c
            .query_row(
                "SELECT 1 FROM git_objects WHERE oid = ?1",
                [&oid.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        Ok(n.is_some())
    }

    pub fn kind(&self, oid: &Oid) -> Result<Option<Kind>> {
        let k: Option<i64> = self
            .c
            .query_row(
                "SELECT kind FROM git_objects WHERE oid = ?1",
                [&oid.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        k.map(|k| Kind::from_code(k as u8)).transpose()
    }

    pub fn commit(&self, oid: &Oid) -> Result<Commit> {
        match self.read(oid)? {
            (Kind::Commit, d) => Commit::parse(&d),
            (Kind::Tag, d) => {
                let t = Tag::parse(&d)?;
                self.commit(&t.object)
            }
            _ => Err(Error::Invalid(format!("{oid} is not a commit"))),
        }
    }

    pub fn tree(&self, oid: &Oid) -> Result<Vec<TreeEntry>> {
        match self.read(oid)? {
            (Kind::Tree, d) => parse_tree(&d),
            (Kind::Commit, d) => self.tree(&Commit::parse(&d)?.tree),
            _ => Err(Error::Invalid(format!("{oid} is not a tree"))),
        }
    }

    pub fn blob(&self, oid: &Oid) -> Result<Vec<u8>> {
        match self.read(oid)? {
            (Kind::Blob, d) => Ok(d),
            _ => Err(Error::Invalid(format!("{oid} is not a blob"))),
        }
    }

    /// A timestamp for an object labrat creates now in this repository: never earlier than
    /// `min`, and later than anything labrat created here before. On a real server operations
    /// are seconds apart; this keeps date-ordered listings from tying within one second.
    pub fn next_time(&self, now: i64, min: i64) -> Result<i64> {
        let last: i64 = self
            .c
            .query_row(
                "SELECT last_time FROM git_clock WHERE repo_id = ?1",
                [self.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let t = now.max(min).max(last + 1);
        self.c.execute(
            "INSERT INTO git_clock (repo_id, last_time) VALUES (?1, ?2)
             ON CONFLICT (repo_id) DO UPDATE SET last_time = excluded.last_time",
            params![self.id, t],
        )?;
        Ok(t)
    }

    /// Writes a commit created by labrat itself, with its committer time from [`Repo::next_time`].
    pub fn write_commit(&self, commit: &Commit) -> Result<Oid> {
        let mut c = commit.clone();
        let max_parent = c
            .parents
            .iter()
            .filter_map(|p| self.commit(p).ok())
            .map(|pc| pc.committer.time)
            .max();
        let t = self.next_time(c.committer.time, max_parent.map(|m| m + 1).unwrap_or(0))?;
        if c.author.time == c.committer.time {
            c.author.time = t;
        }
        c.committer.time = t;
        self.write(Kind::Commit, &c.encode())
    }

    // Refs

    pub fn refs(&self) -> Result<Vec<(String, Oid)>> {
        let mut st = self
            .c
            .prepare("SELECT name, oid FROM git_refs WHERE repo_id = ?1 ORDER BY name")?;
        let rows = st
            .query_map([self.id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(n, o)| Ok((n, Oid::from_bytes(&o)?)))
            .collect()
    }

    pub fn refs_with_prefix(&self, prefix: &str) -> Result<Vec<(String, Oid)>> {
        Ok(self
            .refs()?
            .into_iter()
            .filter(|(n, _)| n.starts_with(prefix))
            .collect())
    }

    pub fn ref_oid(&self, name: &str) -> Result<Option<Oid>> {
        let o: Option<Vec<u8>> = self
            .c
            .query_row(
                "SELECT oid FROM git_refs WHERE repo_id = ?1 AND name = ?2",
                params![self.id, name],
                |r| r.get(0),
            )
            .optional()?;
        o.map(|o| Oid::from_bytes(&o)).transpose()
    }

    pub fn set_ref(&self, name: &str, oid: &Oid) -> Result<()> {
        self.c.execute(
            "INSERT INTO git_refs (repo_id, name, oid) VALUES (?1, ?2, ?3)
             ON CONFLICT (repo_id, name) DO UPDATE SET oid = excluded.oid",
            params![self.id, name, &oid.0[..]],
        )?;
        Ok(())
    }

    pub fn delete_ref(&self, name: &str) -> Result<()> {
        self.c.execute(
            "DELETE FROM git_refs WHERE repo_id = ?1 AND name = ?2",
            params![self.id, name],
        )?;
        Ok(())
    }

    pub fn delete_all_refs(&self) -> Result<()> {
        self.c
            .execute("DELETE FROM git_refs WHERE repo_id = ?1", [self.id])?;
        Ok(())
    }

    pub fn branch(&self, name: &str) -> Result<Option<Oid>> {
        self.ref_oid(&format!("refs/heads/{name}"))
    }

    pub fn branches(&self) -> Result<Vec<(String, Oid)>> {
        Ok(self
            .refs_with_prefix("refs/heads/")?
            .into_iter()
            .map(|(n, o)| (n["refs/heads/".len()..].to_string(), o))
            .collect())
    }

    pub fn tags(&self) -> Result<Vec<(String, Oid)>> {
        Ok(self
            .refs_with_prefix("refs/tags/")?
            .into_iter()
            .map(|(n, o)| (n["refs/tags/".len()..].to_string(), o))
            .collect())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.branches()?.is_empty())
    }

    /// Peels tags until a non-tag object.
    pub fn peel(&self, oid: &Oid) -> Result<Oid> {
        let mut cur = *oid;
        for _ in 0..16 {
            match self.kind(&cur)? {
                Some(Kind::Tag) => cur = Tag::parse(&self.read(&cur)?.1)?.object,
                _ => return Ok(cur),
            }
        }
        Ok(cur)
    }

    /// Resolves a branch, tag, full ref or (abbreviated) sha to a commit id.
    pub fn resolve(&self, rev: &str) -> Result<Option<Oid>> {
        if rev.is_empty() {
            return Ok(None);
        }
        for name in [
            format!("refs/heads/{rev}"),
            format!("refs/tags/{rev}"),
            rev.to_string(),
        ] {
            if let Some(o) = self.ref_oid(&name)? {
                return Ok(Some(self.peel(&o)?));
            }
        }
        if rev.len() == 40 {
            if let Ok(o) = Oid::from_hex(rev) {
                if self.exists(&o)? {
                    return Ok(Some(self.peel(&o)?));
                }
            }
            return Ok(None);
        }
        if rev.len() >= 4 && rev.len() < 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
            // Abbreviated sha: prefix match on the hex form.
            let lo = rev.to_lowercase();
            let mut st = self
                .c
                .prepare("SELECT oid FROM git_objects WHERE hex(oid) LIKE ?1 LIMIT 2")?;
            let rows: Vec<Vec<u8>> = st
                .query_map([format!("{}%", lo.to_uppercase())], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if rows.len() == 1 {
                return Ok(Some(self.peel(&Oid::from_bytes(&rows[0])?)?));
            }
        }
        Ok(None)
    }

    /// Looks up the entry at `path` (slash-separated) under `tree`.
    pub fn entry_at(&self, tree: &Oid, path: &str) -> Result<Option<TreeEntry>> {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return Ok(Some(TreeEntry {
                mode: crate::object::MODE_TREE,
                name: String::new(),
                oid: *tree,
            }));
        }
        let mut cur = *tree;
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            let entries = self.tree(&cur)?;
            let Some(e) = entries.into_iter().find(|e| e.name == *part) else {
                return Ok(None);
            };
            if i == parts.len() - 1 {
                return Ok(Some(e));
            }
            if !e.is_tree() {
                return Ok(None);
            }
            cur = e.oid;
        }
        Ok(None)
    }
}
