//! One SQLite connection behind a mutex. labrat is a test tool, so all access is
//! serialized; that keeps behavior deterministic and is fast enough.

use crate::{Error, Result};
use parking_lot::Mutex;
use rusqlite::Connection;

/// Bump when the schema changes; a database with another version is rejected.
pub const SCHEMA_VERSION: i64 = 24;

pub struct Db {
    conn: Mutex<Connection>,
    /// This instance's clock offset, applied to `time::now()` inside `read`/`tx`.
    offset_ms: std::sync::atomic::AtomicI64,
}

impl Db {
    /// Opens (or creates) the database. `":memory:"` keeps everything in RAM.
    pub fn open(path: &str) -> Result<Db> {
        let conn = if path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            Connection::open(path)?
        };
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = OFF;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(crate::schema::SCHEMA)?;
            conn.execute_batch(labrat_git::store::SCHEMA)?;
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
        } else if version != SCHEMA_VERSION {
            return Err(Error::Internal(anyhow::anyhow!(
                "database schema version {version} does not match {SCHEMA_VERSION}; delete the database file"
            )));
        }
        Ok(Db {
            conn: Mutex::new(conn),
            offset_ms: std::sync::atomic::AtomicI64::new(0),
        })
    }

    /// Runs `f` inside a transaction; it is rolled back if `f` fails.
    pub fn tx<T, E: From<Error>>(
        &self,
        f: impl FnOnce(&Connection) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(Error::from)?;
        let out = crate::time::with_local_offset(self.offset(), || f(&tx))?;
        tx.commit().map_err(Error::from)?;
        Ok(out)
    }

    /// Read-only access without a transaction.
    pub fn read<T, E>(
        &self,
        f: impl FnOnce(&Connection) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let conn = self.conn.lock();
        crate::time::with_local_offset(self.offset(), || f(&conn))
    }

    pub fn offset(&self) -> i64 {
        self.offset_ms.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Shifts this instance's clock: `ms` is added to the system time.
    pub fn set_offset(&self, ms: i64) {
        self.offset_ms.store(ms, std::sync::atomic::Ordering::Relaxed);
    }

    /// The current time on this instance's clock.
    pub fn now(&self) -> i64 {
        crate::time::with_local_offset(self.offset(), crate::time::now)
    }

    /// Deletes all data and recreates the schema.
    pub fn reset(&self) -> Result<()> {
        let conn = self.conn.lock();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        for t in tables {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{t}\";"))?;
        }
        conn.execute_batch(crate::schema::SCHEMA)?;
        conn.execute_batch(labrat_git::store::SCHEMA)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    }

    /// Writes a consistent copy of the database to `path`.
    pub fn snapshot(&self, path: &str) -> Result<()> {
        let conn = self.conn.lock();
        let _ = std::fs::remove_file(path);
        conn.execute("VACUUM INTO ?1", [path])?;
        Ok(())
    }

    /// Replaces the current contents with a snapshot made by [`Db::snapshot`].
    pub fn restore(&self, path: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let src = Connection::open(path)?;
        let backup = rusqlite::backup::Backup::new(&src, &mut conn)?;
        backup.run_to_completion(1000, std::time::Duration::ZERO, None)?;
        Ok(())
    }
}

/// Helpers for optional single-row queries.
pub trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}
