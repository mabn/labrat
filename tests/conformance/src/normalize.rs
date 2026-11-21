//! Replaces values that legitimately differ between GitLab and labrat with placeholders.

use regex::Regex;
use serde_json::json;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

static TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2}| UTC| [+-]\d{4})").unwrap()
});
static GRAVATAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/avatar/[0-9a-f]{64}").unwrap());
static PAT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"glpat-[A-Za-z0-9_\-.]+").unwrap());
static SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{40}\b").unwrap());
static SHORT_SHA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[0-9a-f]{7,12}\b").unwrap());
/// A commit in an "added N commits" system note: `<li>abcdef12 - Title</li>`.
static COMMIT_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(<li>|\.\.\.)([0-9a-f]{8}) - ").unwrap());
static SSH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(ssh://)?git@[^/:]+(:\d+)?[:/]").unwrap());
static RUNNERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"GR1348941[A-Za-z0-9_\-]{20}").unwrap());
static API_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/(projects|groups|users|namespaces|members|all|access_tokens|personal_access_tokens|award_emoji|draft_notes|suggestions|notes|versions|hooks|todos|discussions|milestones|pipelines|uploads|statuses|resource_label_events|resource_state_events|resource_milestone_events|resource_state_events|resource_label_events|resource_milestone_events)/(\d+)").unwrap()
});
/// Query params holding user, project or pipeline ids, possibly inside `not[...]`.
static QUERY_REF_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([?&]|%5B)(source_project_id|project_id|assignee_id|reviewer_id|author_id|merge_user_id|pipeline_id)((?:%5D)?)=(\d+)").unwrap()
});
/// Upload secrets (`/uploads/<32 hex>/`) and project ids in upload paths.
static UPLOAD_SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/uploads/([0-9a-f]{32})/").unwrap());
static UPLOAD_PROJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/-/project/(\d+)/").unwrap());
/// Project bot usernames: `project_<project id>_bot_<random hex>`.
static BOT_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(project|group)_(\d+)_bot_([0-9a-f]{32})").unwrap());
static DIFF_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"diff_id=(\d+)").unwrap());
/// `id=N` in query strings of URLs whose path names the resource kind.
static QUERY_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"/(projects|groups|users)/<(\w+):(\d+)>((?:[^?\s<>]|<\w+(?::\d+)?>)*)\?((?:[^<>\s]|<\w+>)*?)\bid=(\d+)").unwrap()
});
static RUBY_OBJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#<(\w+):0x[0-9a-f]{16}").unwrap());
static DELETION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"deletion_scheduled-\d+").unwrap());
static QUERY_ID_PARAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([?&])id=\d+").unwrap());
static IP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$").unwrap());
static CORRELATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9A-Z]{26}").unwrap());

static GID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"gid://gitlab/([A-Za-z:]+)/(\d+)").unwrap());
static MILESTONE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"milestone_id=(\d+)").unwrap());
/// Ids in the `data-*` attributes of rendered GitLab references.
static HTML_DATA_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"data-(project|merge-request|user|label|milestone|group|author|note)="(\d+)""#).unwrap());
static NOTE_ANCHOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#note_(\d+)").unwrap());
static UUID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b").unwrap());
static HTTP_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][a-z]{2}, \d{2} [A-Z][a-z]{2} \d{4} \d{2}:\d{2}:\d{2} GMT").unwrap());

pub struct Normalizer {
    /// Literal replacements applied first (e.g. a webhook receiver URL).
    pub aliases: Vec<(String, String)>,
    base: String,
    run: String,
    today: String,
    ids: HashMap<(String, i64), usize>,
    counters: HashMap<String, usize>,
    shas: HashMap<String, usize>,
    /// Short SHAs in "added N commits" lists whose full SHA never appeared.
    short_only: HashMap<String, usize>,
    secrets: HashMap<String, usize>,
}

/// Maps a field name to the kind of id it holds.
fn id_kind(field: &str, parent_kind: &str) -> Option<String> {
    let k = match field {
        "id" => return Some(parent_kind.to_string()),
        "iid" | "project_access_level" | "state_id" => return None,
        "user_id" | "author_id" | "creator_id" | "created_by_id" | "assignee_id"
        | "merge_user_id" | "owner_id" | "updated_by_id" | "last_edited_by_id"
        | "resolved_by_id" => "user",
        "project_id" | "source_project_id" | "target_project_id" => "project",
        // GraphQL camelCase ids.
        "projectId" | "sourceProjectId" | "targetProjectId" => "project",
        "noteableId" => "mr",
        "pipeline_id" | "head_pipeline_id" => "pipeline",
        "namespace_id" | "parent_id" | "group_id" => "namespace",
        f if f.ends_with("_id") => return Some(format!("other:{f}")),
        _ => return None,
    };
    Some(k.to_string())
}

/// The kind of object found under a key.
fn object_kind(key: &str, parent_kind: &str) -> String {
    match key {
        "author" | "created_by" | "user" | "assignee" | "merged_by" | "merge_user" | "owner"
        | "closed_by" | "resolved_by" | "reviewer" | "approved_by" | "assignees" | "reviewers" => {
            "user".into()
        }
        "namespace" | "parent" => "namespace".into(),
        "project"
        | "projects"
        | "shared_projects"
        | "source_project"
        | "target_project"
        | "forked_from_project" => "project".into(),
        "_links"
        | "permissions"
        | "project_access"
        | "group_access"
        | "container_expiration_policy" => {
            format!("{parent_kind}.{key}")
        }
        _ => format!("{parent_kind}.{key}"),
    }
}

impl Normalizer {
    /// The day `<TODAY>` stands for (the recording day when replaying).
    pub fn set_today(&mut self, day: &str) {
        self.today = day.to_string();
    }

    pub fn new(base: &str, run: &str) -> Normalizer {
        Normalizer {
            aliases: vec![],
            base: base.trim_end_matches('/').to_string(),
            run: run.to_string(),
            today: labrat_core::time::today(),
            ids: HashMap::new(),
            counters: HashMap::new(),
            shas: HashMap::new(),
            short_only: HashMap::new(),
            secrets: HashMap::new(),
        }
    }

    fn id(&mut self, kind: &str, value: i64) -> String {
        let kind = match kind {
            "projects" => "project",
            "groups" | "namespaces" => "namespace",
            "users" => "user",
            k => k,
        };
        let key = (kind.to_string(), value);
        let n = match self.ids.get(&key) {
            Some(n) => *n,
            None => {
                let c = self.counters.entry(kind.to_string()).or_insert(0);
                *c += 1;
                self.ids.insert(key, *c);
                *c
            }
        };
        format!("<{kind}:{n}>")
    }

    pub fn string(&mut self, s: &str, _ctx: &str) -> String {
        let mut out = s.to_string();
        for (from, to) in &self.aliases {
            out = out.replace(from, to);
        }
        let mut out = out.replace(&self.base, "<BASE>");
        out = UUID.replace_all(&out, "<UUID>").into_owned();
        out = HTTP_DATE.replace_all(&out, "<HTTP_DATE>").into_owned();
        let enc_base = self.base.replace(':', "%3A").replace('/', "%2F");
        out = out.replace(&enc_base, "<BASE>");
        out = out.replace(&self.run, "<RUN>");
        out = TIME.replace_all(&out, "<TIME>").into_owned();
        out = out.replace(&self.today, "<TODAY>");
        out = GRAVATAR.replace_all(&out, "/avatar/<HASH>").into_owned();
        out = PAT.replace_all(&out, "<TOKEN>").into_owned();
        out = SSH.replace_all(&out, "<SSH>/").into_owned();
        out = RUNNERS.replace_all(&out, "<RUNNERS_TOKEN>").into_owned();
        let shas: Vec<String> = SHA
            .find_iter(&out)
            .map(|m| m.as_str().to_string())
            .collect();
        for sha in shas {
            let len = self.shas.len();
            let n = *self.shas.entry(sha.clone()).or_insert(len + 1);
            out = out.replace(&sha, &format!("<SHA{n}>"));
        }
        // Intermediate commits listed in system notes may never appear in full.
        let unknown: Vec<String> = COMMIT_ITEM
            .captures_iter(&out)
            .map(|c| c[2].to_string())
            .filter(|short| !self.shas.keys().any(|full| full.starts_with(short.as_str())))
            .collect();
        for short in unknown {
            let len = self.short_only.len();
            let n = *self.short_only.entry(short.clone()).or_insert(len + 1);
            out = out.replace(&format!("<li>{short} - "), &format!("<li><SHORTSHA{n}> - "));
            out = out.replace(&format!("...{short} - "), &format!("...<SHORTSHA{n}> - "));
        }
        out = SHORT_SHA
            .replace_all(&out, |caps: &regex::Captures| {
                let short = &caps[0];
                match self.shas.iter().find(|(full, _)| full.starts_with(short)) {
                    Some((_, n)) => format!("<SHA{n}:short>"),
                    None => short.to_string(),
                }
            })
            .into_owned();
        let gids: Vec<(String, String, i64)> = GID
            .captures_iter(&out)
            .map(|c| (c[0].to_string(), c[1].to_string(), c[2].parse().unwrap()))
            .collect();
        for (whole, kind, id) in gids {
            let k = match kind.as_str() {
                "User" => "user",
                "Project" => "project",
                "Group" | "Namespaces::UserNamespace" | "Namespace" => "namespace",
                "MergeRequest" => "mr",
                "Note" | "DiffNote" | "DiscussionNote" => "note",
                "AwardEmoji" => "award",
                "ProjectLabel" | "GroupLabel" | "Label" => "label",
                other => other,
            };
            let ph = self.id(k, id);
            out = out.replacen(&whole, &format!("gid://gitlab/{kind}/{ph}"), 1);
        }
        let anchors: Vec<i64> = NOTE_ANCHOR.captures_iter(&out).map(|c| c[1].parse().unwrap()).collect();
        for a in anchors {
            let ph = self.id("note", a);
            out = out.replacen(&format!("#note_{a}"), &format!("#note_{ph}"), 1);
        }
        let ms_ids: Vec<i64> = MILESTONE_ID.captures_iter(&out).map(|c| c[1].parse().unwrap()).collect();
        for m in ms_ids {
            let ph = self.id("milestone", m);
            out = out.replacen(&format!("milestone_id={m}"), &format!("milestone_id={ph}"), 1);
        }
        let data_ids: Vec<(String, String, i64)> = HTML_DATA_ID
            .captures_iter(&out)
            .map(|c| (c[0].to_string(), c[1].to_string(), c[2].parse().unwrap()))
            .collect();
        for (whole, kind, id) in data_ids {
            let k = match kind.as_str() {
                "merge-request" => "mr",
                "user" | "author" => "user",
                "group" => "namespace",
                other => other,
            };
            let ph = self.id(k, id);
            out = out.replacen(&whole, &format!("data-{kind}=\"{ph}\""), 1);
        }
        let refs: Vec<(String, String, i64)> = QUERY_REF_ID
            .captures_iter(&out)
            .map(|c| (c[0].to_string(), c[2].to_string(), c[4].parse().unwrap()))
            .collect();
        for (whole, key, id) in refs {
            let kind = if key.ends_with("project_id") {
                "project"
            } else if key == "pipeline_id" {
                "pipeline"
            } else {
                "user"
            };
            let ph = self.id(kind, id);
            let new = whole.replacen(&format!("={id}"), &format!("={ph}"), 1);
            out = out.replacen(&whole, &new, 1);
        }
        let secrets: Vec<String> = UPLOAD_SECRET.captures_iter(&out).map(|c| c[1].to_string()).collect();
        for sec in secrets {
            let n = {
                let next = self.secrets.len() + 1;
                *self.secrets.entry(sec.clone()).or_insert(next)
            };
            out = out.replace(&format!("/uploads/{sec}/"), &format!("/uploads/<SECRET:{n}>/"));
        }
        let bots: Vec<(String, String, i64, String)> =
            BOT_NAME.captures_iter(&out).map(|c| (c[0].to_string(), c[1].to_string(), c[2].parse().unwrap(), c[3].to_string())).collect();
        for (whole, kind, rid, hash) in bots {
            let ph = self.id(if kind == "group" { "namespace" } else { "project" }, rid);
            let n = {
                let next = self.secrets.len() + 1;
                *self.secrets.entry(hash.clone()).or_insert(next)
            };
            out = out.replace(&whole, &format!("{kind}_{ph}_bot_<SECRET:{n}>"));
        }
        let up_projects: Vec<i64> = UPLOAD_PROJECT.captures_iter(&out).map(|c| c[1].parse().unwrap()).collect();
        for id in up_projects {
            let ph = self.id("project", id);
            out = out.replacen(&format!("/-/project/{id}/"), &format!("/-/project/{ph}/"), 1);
        }
        let diff_ids: Vec<i64> = DIFF_ID
            .captures_iter(&out)
            .map(|c| c[1].parse().unwrap())
            .collect();
        for d in diff_ids {
            let ph = self.id("diff", d);
            out = out.replacen(&format!("diff_id={d}"), &format!("diff_id={ph}"), 1);
        }
        // UI paths (`/-/milestones/1`) carry iids, not ids.
        let ids: Vec<(String, String, i64)> = API_ID
            .captures_iter(&out)
            .filter(|c| !(c[1].eq("milestones") && out[..c.get(0).unwrap().start()].ends_with("/-")))
            .map(|c| (c[0].to_string(), c[1].to_string(), c[2].parse().unwrap()))
            .collect();
        for (whole, kind, id) in ids {
            let id_kind = match kind.as_str() {
                "members" | "all" => "user",
                "award_emoji" => "award",
                "access_tokens" | "personal_access_tokens" => "token",
                "draft_notes" => "draft",
                "suggestions" => "suggestion",
                "notes" | "discussions" => "note",
                "milestones" => "milestone",
                "pipelines" => "pipeline",
                "resource_label_events" => "label_event",
                "resource_state_events" => "state_event",
                "resource_milestone_events" => "milestone_event",
                "versions" => "version",
                "uploads" => "upload",
                k => k,
            };
            let ph = self.id(id_kind, id);
            out = out.replacen(&whole, &format!("/{kind}/{ph}"), 1);
        }
        let queries: Vec<(String, String, String)> = QUERY_ID
            .captures_iter(&out)
            .map(|c| (c[0].to_string(), c[2].to_string(), c[6].to_string()))
            .collect();
        for (whole, kind, id) in queries {
            let ph = self.id(&kind, id.parse().unwrap());
            let replaced = QUERY_ID_PARAM
                .replace(&whole, |c: &regex::Captures| format!("{}id={ph}", &c[1]))
                .into_owned();
            out = out.replace(&whole, &replaced);
        }
        out = DELETION
            .replace_all(&out, "deletion_scheduled-<ID>")
            .into_owned();
        // Ruby's `inspect` of an object includes its address (`#<GlobalID:0x0000...`).
        out = RUBY_OBJECT.replace_all(&out, "#<$1:<ADDR>").into_owned();
        if IP.is_match(&out) {
            out = "<IP>".into();
        }
        out
    }

    pub fn value(&mut self, v: &mut Value, ctx: &str) {
        self.value_kind(v, ctx)
    }

    pub fn value_kind(&mut self, v: &mut Value, kind: &str) {
        match v {
            Value::String(s) => {
                let n = self.string(s, kind);
                let n = if kind == "header" && n.contains("correlation_id") {
                    CORRELATION.replace_all(&n, "<ID>").into_owned()
                } else {
                    n
                };
                *s = n;
            }
            Value::Array(a) => {
                for item in a {
                    self.value_kind(item, kind);
                }
            }
            Value::Object(m) => {
                // Webhook payloads: `object_attributes` holds the object named by `object_kind`.
                let hook_kind = m.get("object_kind").and_then(Value::as_str).map(|k| match k {
                    "merge_request" => "mr".to_string(),
                    "emoji" => "award".to_string(),
                    other => other.to_string(),
                });
                // Award emoji: ids of different awardable types may coincide in labrat, so the
                // placeholder is keyed by type and id (keeping the `other:awardable_id` naming).
                if let (Some(t), Some(n)) = (m.get("awardable_type").and_then(Value::as_str), m.get("awardable_id").and_then(Value::as_i64)) {
                    let code = match t {
                        "MergeRequest" => 1,
                        "Note" | "DiffNote" | "DiscussionNote" => 2,
                        _ => 3,
                    };
                    let v = self.id("other:awardable_id", n * 8 + code);
                    m.insert("awardable_id".to_string(), Value::String(v));
                }
                // Events: `target_id`/`target_iid` hold ids of the `target_type`.
                if let Some(tt) = m.get("target_type").and_then(Value::as_str).map(str::to_string) {
                    let k = match tt.as_str() {
                        "Project" => Some("project"),
                        "MergeRequest" => Some("mr"),
                        "Note" | "DiffNote" | "DiscussionNote" => Some("note"),
                        _ => None,
                    };
                    if let Some(k) = k {
                        for key in ["target_id", "target_iid"] {
                            if key == "target_iid" && tt == "MergeRequest" {
                                continue;
                            }
                            if let Some(n) = m.get(key).and_then(Value::as_i64) {
                                m.insert(key.to_string(), Value::String(self.id(k, n)));
                            }
                        }
                    }
                }
                let keys: Vec<String> = m.keys().cloned().collect();
                for k in keys {
                    let entry = m.get_mut(&k).unwrap();
                    // On-disk repository sizes depend on how Git packed the objects.
                    if k == "root_repository_size" && entry.is_number() {
                        *entry = json!("<BYTES>");
                        continue;
                    }
                    if matches!(k.as_str(), "cursor" | "startCursor" | "endCursor") {
                        if let Some(c) = entry.as_str() {
                            use base64::Engine;
                            if let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(c.trim_end_matches('=')) {
                                let decoded = String::from_utf8_lossy(&raw).into_owned();
                                let mut v: Value = serde_json::from_str(&decoded).unwrap_or(Value::String(decoded));
                                if let Value::Object(m) = &mut v {
                                    for (_, x) in m.iter_mut() {
                                        if let Some(n) = x.as_str().and_then(|s| s.parse::<i64>().ok()) {
                                            *x = Value::String(self.id("cursor", n));
                                        }
                                    }
                                }
                                self.value_kind(&mut v, "cursor");
                                *entry = json!({"cursor": v});
                            }
                        }
                        continue;
                    }
                    // Wall-clock dependent: how long a pipeline waited before starting.
                    if k == "queued_duration" && kind != "hook" {
                        *entry = Value::String("<QUEUED>".into());
                        continue;
                    }
                    if k == "execution_duration" && entry.is_number() {
                        *entry = Value::String("<DURATION>".into());
                        continue;
                    }
                    if let Value::Number(n) = entry
                        && let (Some(i), Some(idk)) = (n.as_i64(), id_kind(&k, kind)) {
                            *entry = Value::String(self.id(&idk, i));
                            continue;
                        }
                    // `changes` entries: {"previous": id, "current": id} under an id field.
                    if let Value::Object(pc) = entry
                        && pc.contains_key("previous") && pc.contains_key("current")
                            && let Some(idk) = id_kind(&k, kind).filter(|_| k != "id") {
                                for side in ["previous", "current"] {
                                    if let Some(i) = pc[side].as_i64() {
                                        pc[side] = Value::String(self.id(&idk, i));
                                    }
                                }
                                continue;
                            }
                    // EE groups: user ids alerted about excessive project downloads.
                    if let Value::Array(a) = entry
                        && k == "unique_project_download_limit_alertlist" {
                            for item in a.iter_mut() {
                                if let Some(i) = item.as_i64() {
                                    *item = Value::String(self.id("user", i));
                                }
                            }
                            continue;
                        }
                    if let Value::Array(a) = entry
                        && k.ends_with("_ids") {
                            let idk = id_kind(&k[..k.len() - 1], kind).unwrap_or(k.clone());
                            for item in a.iter_mut() {
                                if let Some(i) = item.as_i64() {
                                    *item = Value::String(self.id(&idk, i));
                                } else {
                                    self.value_kind(item, kind);
                                }
                            }
                            continue;
                        }
                    let child = match (k.as_str(), &hook_kind) {
                        ("object_attributes", Some(hk)) => hk.clone(),
                        ("merge_request", _) => "mr".to_string(),
                        ("note", _) if entry.is_object() => "note".to_string(),
                        _ => object_kind(&k, kind),
                    };
                    self.value_kind(entry, &child);
                }
            }
            _ => {}
        }
    }
}

/// Returns a description of the first difference, comparing key order too.
pub fn diff(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(a), Value::Object(b)) => {
            let ka: Vec<&String> = a.keys().collect();
            let kb: Vec<&String> = b.keys().collect();
            for k in &ka {
                if !b.contains_key(*k) {
                    return Some(format!("{path}.{k}: missing (expected {})", short(&a[*k])));
                }
            }
            for k in &kb {
                if !a.contains_key(*k) {
                    return Some(format!(
                        "{path}.{k}: unexpected field (value {})",
                        short(&b[*k])
                    ));
                }
            }
            for k in &ka {
                if let Some(d) = diff(&a[*k], &b[*k], &format!("{path}.{k}")) {
                    return Some(d);
                }
            }
            if ka != kb {
                return Some(format!(
                    "{path}: key order differs\n    expected {ka:?}\n    actual   {kb:?}"
                ));
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!(
                    "{path}: expected {} items, got {}",
                    a.len(),
                    b.len()
                ));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(d) = diff(x, y, &format!("{path}[{i}]")) {
                    return Some(d);
                }
            }
            None
        }
        (a, b) if a == b => None,
        (a, b) => Some(format!("{path}: expected {}, got {}", short(a), short(b))),
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 300 {
        format!("{}…", &s[..300])
    } else {
        s
    }
}
