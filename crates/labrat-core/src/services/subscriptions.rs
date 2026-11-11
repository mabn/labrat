//! Explicit and implicit subscriptions to merge requests.

use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};

/// Explicit subscription state, if any.
pub fn explicit(c: &Connection, user_id: i64, kind: &str, id: i64) -> Result<Option<bool>> {
    Ok(c.query_row(
        "SELECT subscribed FROM subscriptions WHERE user_id = ?1 AND subscribable_type = ?2 AND subscribable_id = ?3",
        params![user_id, kind, id],
        |r| r.get(0),
    )
    .optional()?)
}

pub fn set(c: &Connection, user_id: i64, kind: &str, id: i64, subscribed: bool) -> Result<()> {
    c.execute(
        "INSERT INTO subscriptions (user_id, subscribable_type, subscribable_id, subscribed) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (user_id, subscribable_type, subscribable_id) DO UPDATE SET subscribed = excluded.subscribed",
        params![user_id, kind, id, subscribed],
    )?;
    Ok(())
}

/// Participants (author, assignees, reviewers, commenters) are subscribed unless they opted out.
pub fn ensure_participant(_c: &Connection, _user_id: i64, _kind: &str, _id: i64) -> Result<()> {
    Ok(())
}

/// `Participable#raw_participants` for a merge request as seen by `viewer`, in GitLab's
/// order: reviewers, assignees, notes oldest first (each note's author, then whoever awarded
/// it), the author, the MR's award givers, then @mentioned users (title and description first,
/// then the notes'). Users who can't read the
/// MR are dropped (`visible_participants`).
pub fn participant_ids(
    c: &Connection,
    p: &crate::model::Project,
    mr: &crate::services::merge_requests::MergeRequest,
    viewer: Option<&crate::model::User>,
) -> Result<Vec<i64>> {
    use crate::services::mentions::mentioned_usernames;
    use crate::services::notes::Note;
    let mut ids: Vec<i64> = vec![];
    let add = |id: i64, ids: &mut Vec<i64>| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };
    let awarders = |kind: &str, id: i64| -> Result<Vec<i64>> {
        let mut st = c.prepare("SELECT user_id FROM award_emoji WHERE awardable_type = ?1 AND awardable_id = ?2 ORDER BY id")?;
        let v = st.query_map(params![kind, id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(v)
    };
    let reads_internal = match viewer {
        Some(u) => {
            let perms = crate::perms::ProjectPerms::load(c, Some(u), p)?;
            perms.reporter() || perms.admin
        }
        None => false,
    };
    let mut mentioned = mentioned_usernames(&crate::services::todos::mentionable_text(mr));
    let note_mention = |text: &str, mentioned: &mut Vec<String>| {
        for m in mentioned_usernames(text) {
            if !mentioned.contains(&m) {
                mentioned.push(m);
            }
        }
    };
    for id in mr.reviewer_ids(c)? {
        add(id, &mut ids);
    }
    for id in mr.assignee_ids(c)? {
        add(id, &mut ids);
    }
    let notes: Vec<Note> = Note::for_noteable(c, "MergeRequest", mr.id)?.into_iter().filter(|n| !n.internal || reads_internal).take(200).collect();
    // Notes in order (`notes_for_participants`): each note's author, then its award givers;
    // system notes contribute their author and their mentions.
    for n in &notes {
        add(n.author_id, &mut ids);
        for a in awarders("Note", n.id)? {
            add(a, &mut ids);
        }
        note_mention(&n.note, &mut mentioned);
    }
    add(mr.author_id, &mut ids);
    for a in awarders("MergeRequest", mr.id)? {
        add(a, &mut ids);
    }
    for name in mentioned {
        for u in crate::services::mentions::users_for_mention(c, &name)? {
            add(u.id, &mut ids);
        }
    }
    let mut visible = vec![];
    for id in ids {
        if let Some(u) = crate::model::User::find(c, id)?
            && crate::perms::ProjectPerms::load(c, Some(&u), p)?.read_merge_request(p)
        {
            visible.push(id);
        }
    }
    Ok(visible)
}

/// `subscribed?(user)` for a merge request: an explicit subscription, else `participant?`.
pub fn subscribed_to_mr(
    c: &Connection,
    user_id: i64,
    mr: &crate::services::merge_requests::MergeRequest,
) -> Result<bool> {
    if let Some(s) = explicit(c, user_id, "MergeRequest", mr.id)? {
        return Ok(s);
    }
    let Some(user) = crate::model::User::find(c, user_id)? else { return Ok(false) };
    let Some(p) = crate::model::Project::find(c, mr.target_project_id)? else { return Ok(false) };
    Ok(participant_ids(c, &p, mr, Some(&user))?.contains(&user_id))
}
