//! Merge request diff versions (`merge_request_diffs`) and the `refs/merge-requests/:iid/head` ref.

use super::MergeRequest;
use crate::db::OptionalExt;
use crate::model::Project;
use crate::{Result, repo, time};
use labrat_git::{Oid, Repo, walk};
use rusqlite::{Connection, Row, params};

#[derive(Debug, Clone)]
pub struct Diff {
    pub id: i64,
    pub merge_request_id: i64,
    pub state: String,
    pub base_commit_sha: Option<String>,
    pub start_commit_sha: Option<String>,
    pub head_commit_sha: Option<String>,
    pub real_size: Option<String>,
    pub commits_count: i64,
    pub patch_id_sha: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str = "id, merge_request_id, state, base_commit_sha, start_commit_sha, head_commit_sha, real_size, commits_count, patch_id_sha, created_at";

impl Diff {
    fn from_row(r: &Row) -> rusqlite::Result<Diff> {
        Ok(Diff {
            id: r.get(0)?,
            merge_request_id: r.get(1)?,
            state: r.get(2)?,
            base_commit_sha: r.get(3)?,
            start_commit_sha: r.get(4)?,
            head_commit_sha: r.get(5)?,
            real_size: r.get(6)?,
            commits_count: r.get(7)?,
            patch_id_sha: r.get(8)?,
            created_at: r.get(9)?,
        })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Diff>> {
        c.query_row(
            &format!("SELECT {COLUMNS} FROM merge_request_diffs WHERE id = ?1"),
            [id],
            Self::from_row,
        )
        .optional()
    }

    /// All versions of an MR, newest first.
    pub fn for_mr(c: &Connection, mr_id: i64) -> Result<Vec<Diff>> {
        let mut st = c.prepare(&format!(
            "SELECT {COLUMNS} FROM merge_request_diffs WHERE merge_request_id = ?1 ORDER BY id DESC"
        ))?;
        let v = st
            .query_map([mr_id], Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    pub fn oids(&self) -> (Option<Oid>, Option<Oid>, Option<Oid>) {
        let p = |s: &Option<String>| s.as_deref().and_then(|x| Oid::from_hex(x).ok());
        (
            p(&self.base_commit_sha),
            p(&self.start_commit_sha),
            p(&self.head_commit_sha),
        )
    }

    /// Commits in this version, newest first.
    pub fn commits(&self, r: &Repo) -> Result<Vec<Oid>> {
        let (base, _, head) = self.oids();
        let Some(head) = head else { return Ok(vec![]) };
        Ok(walk::rev_list(
            r,
            &[head],
            &base.into_iter().collect::<Vec<_>>(),
        )?)
    }

    /// File changes between base and head.
    pub fn changes(&self, r: &Repo) -> Result<Vec<labrat_git::diff::FileChange>> {
        let (base, _, head) = self.oids();
        let (Some(head), true) = (head, self.state != "empty") else {
            return Ok(vec![]);
        };
        let base_tree = match base {
            Some(b) => Some(r.commit(&b)?.tree),
            None => None,
        };
        let head_tree = r.commit(&head)?.tree;
        Ok(labrat_git::diff::diff_trees(
            r,
            base_tree.as_ref(),
            Some(&head_tree),
        )?)
    }
}

/// `MergeRequest#create_merge_request_diff`: snapshots the current source and target heads,
/// updates `refs/merge-requests/:iid/head` and makes this the latest version.
/// `ensure_merge_request_diff` on creation: when the source branch is missing GitLab still
/// stores an empty version that knows only the target head (`start_sha`).
pub fn ensure_version(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Option<Diff>> {
    if let Some(d) = create_version(c, p, mr)? {
        return Ok(Some(d));
    }
    let r = repo::repo(c, p);
    let start = r.branch(&mr.target_branch)?;
    let now = time::now();
    c.execute(
        "INSERT INTO merge_request_diffs (merge_request_id, state, base_commit_sha, start_commit_sha, head_commit_sha, real_size, commits_count, patch_id_sha, created_at, updated_at)
         VALUES (?1, 'empty', NULL, ?2, NULL, NULL, 0, NULL, ?3, ?3)",
        params![mr.id, start.map(|o| o.hex()), now],
    )?;
    let id = c.last_insert_rowid();
    c.execute(
        "UPDATE merge_requests SET latest_diff_id = ?1 WHERE id = ?2",
        params![id, mr.id],
    )?;
    Diff::find(c, id)
}

pub fn create_version(c: &Connection, p: &Project, mr: &MergeRequest) -> Result<Option<Diff>> {
    let r = repo::repo(c, p);
    let Some(head) = r.branch(&mr.source_branch)? else {
        return Ok(None);
    };
    let start = r.branch(&mr.target_branch)?;
    r.set_ref(&format!("refs/merge-requests/{}/head", mr.iid), &head)?;
    let base = match start {
        Some(s) => walk::merge_base(&r, &s, &head)?,
        None => None,
    };
    let commits = walk::rev_list(&r, &[head], &base.into_iter().collect::<Vec<_>>())?;
    let mut patch_id = None;
    let (state, real_size) = if commits.is_empty() {
        ("empty", None)
    } else {
        let base_tree = match base {
            Some(b) => Some(r.commit(&b)?.tree),
            None => None,
        };
        let changes =
            labrat_git::diff::diff_trees(&r, base_tree.as_ref(), Some(&r.commit(&head)?.tree))?;
        patch_id =
            labrat_git::diff::patch_id(&labrat_git::diff::full_patch(&r, &changes)?).map(|o| o.hex());
        if changes.len() > 1000 {
            ("overflow", Some("1000+".to_string()))
        } else {
            ("collected", Some(changes.len().to_string()))
        }
    };
    let now = time::now();
    c.execute(
        "INSERT INTO merge_request_diffs (merge_request_id, state, base_commit_sha, start_commit_sha, head_commit_sha, real_size, commits_count, patch_id_sha, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?9, ?8, ?8)",
        params![
            mr.id,
            state,
            base.map(|o| o.hex()),
            start.map(|o| o.hex()),
            head.hex(),
            real_size,
            commits.len() as i64,
            now,
            patch_id
        ],
    )?;
    let id = c.last_insert_rowid();
    c.execute(
        "UPDATE merge_requests SET latest_diff_id = ?1 WHERE id = ?2",
        params![id, mr.id],
    )?;
    Diff::find(c, id)
}
