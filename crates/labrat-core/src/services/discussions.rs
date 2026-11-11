//! Diff discussions across MR versions (`Discussions::UpdateDiffPositionService`).

use crate::model::{Project, User};
use crate::services::merge_requests::{MergeRequest, diffs::Diff};
use crate::services::notes::{self, NewNote, Note};
use crate::services::positions::Position;
use crate::{Config, Result, repo, time};
use rusqlite::{Connection, params};

/// After a new diff version, re-anchors active diff threads or marks them outdated.
pub fn update_diff_positions(
    c: &Connection,
    config: &Config,
    p: &Project,
    mr: &MergeRequest,
    old: Option<&Diff>,
    user: &User,
) -> Result<()> {
    let (Some(old), Some(new)) = (old, mr.latest_diff(c)?) else {
        return Ok(());
    };
    if old.id == new.id {
        return Ok(());
    }
    let r = repo::repo(c, p);
    let (nb, ns, nh) = (
        new.base_commit_sha.clone().unwrap_or_default(),
        new.start_commit_sha.clone().unwrap_or_default(),
        new.head_commit_sha.clone().unwrap_or_default(),
    );
    let old_refs = (
        old.base_commit_sha.clone(),
        old.start_commit_sha.clone(),
        old.head_commit_sha.clone(),
    );
    let version = Diff::for_mr(c, mr.id)?.len();
    let mut resolved_any = false;
    let mut seen = std::collections::HashSet::new();
    for note in Note::for_noteable(c, "MergeRequest", mr.id)? {
        if note.kind.as_deref() != Some("DiffNote")
            || note.system
            || !seen.insert(note.discussion_id.clone())
        {
            continue;
        }
        let Some(pos) = note.position.as_deref().and_then(Position::parse) else {
            continue;
        };
        if pos.diff_refs() != old_refs {
            continue;
        }
        match pos.trace(&r, &nb, &ns, &nh)? {
            Some(traced) => {
                let code = if traced.on_text() {
                    traced.line_code(&r)?
                } else {
                    None
                };
                c.execute(
                    "UPDATE notes SET position = ?1, line_code = coalesce(?2, line_code), change_position = NULL WHERE discussion_id = ?3 AND type = 'DiffNote'",
                    params![traced.store(), code, note.discussion_id],
                )?;
            }
            None => {
                let change = pos.change_position(&r, &nh)?;
                c.execute(
                    "UPDATE notes SET change_position = ?1 WHERE discussion_id = ?2 AND type = 'DiffNote'",
                    params![change.as_ref().unwrap_or(&pos).store(), note.discussion_id],
                )?;
                if p.resolve_outdated_diff_discussions {
                    let n = c.execute(
                        "UPDATE notes SET resolved_at = ?1, resolved_by_id = ?2, resolved_by_push = 1 WHERE discussion_id = ?3 AND resolvable = 1 AND resolved_at IS NULL",
                        params![time::now(), user.id, note.discussion_id],
                    )?;
                    resolved_any |= n > 0;
                }
                let what = if pos.on_text() { "line" } else { "file" };
                // File and image positions anchor at `file_hash`, the SHA1 of the path.
                let anchor_code = if !pos.on_text() {
                    let cp = change.as_ref().unwrap_or(&pos);
                    let path = cp.new_path.clone().filter(|x| !x.is_empty()).or(cp.old_path.clone()).unwrap_or_default();
                    labrat_git::diff::line_code(&path, 0, 0).split('_').next().map(str::to_string)
                } else { match &change {
                    Some(cp) => {
                        let l = cp.old_line.unwrap_or(0) as usize;
                        let path = cp.file_path().unwrap_or_default();
                        let a = cp
                            .line(&r)?
                            .map(|x| labrat_git::diff::line_code(&path, x.old_pos, x.new_pos));
                        a.or_else(|| Some(labrat_git::diff::line_code(&path, l, l)))
                    }
                    None => note.line_code.clone(),
                } };
                let anchor = anchor_code.map(|l| format!("#{l}")).unwrap_or_default();
                let body = format!(
                    "changed this {what} in [version {version} of the diff](/{}/-/merge_requests/{}/diffs?diff_id={}&start_sha={}{anchor})",
                    p.full_path,
                    mr.iid,
                    new.id,
                    old.head_commit_sha.clone().unwrap_or_default(),
                );
                notes::insert(
                    c,
                    user,
                    NewNote {
                        note: body,
                        noteable_type: "MergeRequest".into(),
                        noteable_id: mr.id,
                        project_id: Some(p.id),
                        system: true,
                        system_action: Some("outdated".into()),
                        kind: Some("DiffNote".into()),
                        discussion_id: Some(note.discussion_id.clone()),
                        position: note.position.clone(),
                        original_position: note.original_position.clone(),
                        line_code: note.line_code.clone(),
                        // `discussion.reply_attributes` carry the thread's `commit_id`.
                        commit_id: note.commit_id.clone(),
                        ..Default::default()
                    },
                )?;
            }
        }
    }
    // Suggestions whose lines changed are outdated.
    c.execute(
        "UPDATE suggestions SET outdated = 1 WHERE applied = 0 AND note_id IN (SELECT id FROM notes WHERE noteable_type = 'MergeRequest' AND noteable_id = ?1 AND change_position IS NOT NULL)",
        [mr.id],
    )?;
    // `ResolvedDiscussionNotificationService`: threads resolved by the push may resolve them all.
    if resolved_any {
        crate::services::comments::all_resolved_notification(c, config, p, mr, user)?;
    }
    Ok(())
}
