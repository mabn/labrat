//! A small git implementation on SQLite: objects, packs, the smart HTTP protocol, diffs and
//! merges. Only SHA-1 repositories are supported.

pub mod attributes;
pub mod diff;
pub mod merge;
pub mod object;
pub mod pack;
pub mod protocol;
pub mod store;
pub mod walk;
pub mod xdiff;

pub use object::{Commit, Kind, Oid, Signature, Tag, TreeEntry};
pub use store::Repo;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("object {0} not found")]
    NotFound(Oid),
    #[error("invalid git data: {0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
