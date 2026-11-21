//! Database schema. Tables and columns follow GitLab's own model where practical.
//! Timestamps are milliseconds since the epoch; dates are `YYYY-MM-DD` text.

pub const SCHEMA: &str = r#"
CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    public_email TEXT,
    commit_email TEXT,
    encrypted_password TEXT,
    state TEXT NOT NULL DEFAULT 'active',
    admin INTEGER NOT NULL DEFAULT 0,
    -- 0 human, 1 support bot, 4 project bot, ... (GitLab's user_type enum)
    user_type INTEGER NOT NULL DEFAULT 0,
    external INTEGER NOT NULL DEFAULT 0,
    private_profile INTEGER NOT NULL DEFAULT 0,
    projects_limit INTEGER NOT NULL DEFAULT 100000,
    can_create_group INTEGER NOT NULL DEFAULT 1,
    bio TEXT NOT NULL DEFAULT '',
    location TEXT NOT NULL DEFAULT '',
    linkedin TEXT NOT NULL DEFAULT '',
    twitter TEXT NOT NULL DEFAULT '',
    discord TEXT NOT NULL DEFAULT '',
    website_url TEXT NOT NULL DEFAULT '',
    github TEXT NOT NULL DEFAULT '',
    job_title TEXT NOT NULL DEFAULT '',
    organization TEXT NOT NULL DEFAULT '',
    pronouns TEXT,
    note TEXT,
    theme_id INTEGER NOT NULL DEFAULT 3,
    color_scheme_id INTEGER NOT NULL DEFAULT 1,
    preferred_language TEXT NOT NULL DEFAULT 'en',
    timezone TEXT,
    namespace_id INTEGER,
    created_by_id INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    last_sign_in_at INTEGER,
    current_sign_in_at INTEGER,
    last_activity_on TEXT,
    sign_in_count INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX users_username ON users (lower(username));
CREATE UNIQUE INDEX users_email ON users (lower(email));

CREATE TABLE personal_access_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT,
    token_digest TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL,
    impersonation INTEGER NOT NULL DEFAULT 0,
    revoked INTEGER NOT NULL DEFAULT 0,
    expires_at TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_used_at INTEGER,
    -- Project access tokens: the project and the bot's access level.
    resource_type TEXT,
    resource_id INTEGER,
    access_level INTEGER
);

-- Users' personal namespaces, groups and project namespaces share one id sequence, as in GitLab.
CREATE TABLE namespaces (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL,              -- 'User', 'Group' or 'Project'
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    parent_id INTEGER REFERENCES namespaces (id) ON DELETE CASCADE,
    owner_id INTEGER,
    description TEXT,
    visibility_level INTEGER NOT NULL DEFAULT 0,
    request_access_enabled INTEGER NOT NULL DEFAULT 1,
    project_creation_level INTEGER,
    subgroup_creation_level INTEGER,
    settings TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Full paths of users, groups and projects, used for lookups by path.
CREATE TABLE routes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type TEXT NOT NULL,       -- 'Namespace' or 'Project'
    source_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE (source_type, source_id)
);
CREATE UNIQUE INDEX routes_path ON routes (lower(path));

CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    namespace_id INTEGER NOT NULL REFERENCES namespaces (id),
    project_namespace_id INTEGER NOT NULL REFERENCES namespaces (id),
    description TEXT,
    visibility_level INTEGER NOT NULL DEFAULT 0,
    creator_id INTEGER,
    default_branch TEXT,
    archived INTEGER NOT NULL DEFAULT 0,
    merge_method TEXT NOT NULL DEFAULT 'merge',
    squash_option TEXT NOT NULL DEFAULT 'default_off',
    only_allow_merge_if_pipeline_succeeds INTEGER NOT NULL DEFAULT 0,
    allow_merge_on_skipped_pipeline INTEGER,
    only_allow_merge_if_all_discussions_are_resolved INTEGER NOT NULL DEFAULT 0,
    remove_source_branch_after_merge INTEGER NOT NULL DEFAULT 1,
    printing_merge_request_link_enabled INTEGER NOT NULL DEFAULT 1,
    resolve_outdated_diff_discussions INTEGER NOT NULL DEFAULT 0,
    merge_commit_template TEXT,
    squash_commit_template TEXT,
    suggestion_commit_message TEXT,
    issue_branch_template TEXT,
    request_access_enabled INTEGER NOT NULL DEFAULT 1,
    -- feature access levels: {"merge_requests": 20, "repository": 20, ...}
    features TEXT NOT NULL DEFAULT '{}',
    -- settings labrat only stores and echoes back
    settings TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    last_activity_at INTEGER NOT NULL
);

CREATE TABLE topics (
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    PRIMARY KEY (project_id, name)
);

CREATE TABLE members (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_type TEXT NOT NULL,       -- 'Project' or 'Namespace'
    source_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    access_level INTEGER NOT NULL,
    expires_at TEXT,
    created_by_id INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (source_type, source_id, user_id)
);

CREATE TABLE protected_branches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    allow_force_push INTEGER NOT NULL DEFAULT 0,
    -- EE: merging needs Code Owner approval.
    code_owner_approval_required INTEGER NOT NULL DEFAULT 0,
    -- EE: who may unprotect the branch. EE creates a Maintainers level for branches protected
    -- through the API, and none for default branch protection.
    unprotect_access_level INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE protected_branch_access_levels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    protected_branch_id INTEGER NOT NULL REFERENCES protected_branches (id) ON DELETE CASCADE,
    kind TEXT NOT NULL,              -- 'push' or 'merge'
    access_level INTEGER NOT NULL
);

-- Per-project internal id sequences (MR iids, issue iids, ...).
CREATE TABLE internal_ids (
    project_id INTEGER NOT NULL,
    usage TEXT NOT NULL,
    last_value INTEGER NOT NULL,
    PRIMARY KEY (project_id, usage)
);

CREATE TABLE merge_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    iid INTEGER NOT NULL,
    target_project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    source_project_id INTEGER,
    source_branch TEXT NOT NULL,
    target_branch TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    author_id INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'opened',
    merge_status TEXT NOT NULL DEFAULT 'unchecked',
    discussion_locked INTEGER,
    -- Set when the MR was retargeted because its target branch was merged and deleted.
    retargeted INTEGER NOT NULL DEFAULT 0,
    squash INTEGER NOT NULL DEFAULT 0,
    -- EE: the deprecated per-MR approval count given on create.
    approvals_before_merge INTEGER,
    merge_params TEXT NOT NULL DEFAULT '{}',
    merge_when_pipeline_succeeds INTEGER NOT NULL DEFAULT 0,
    merge_user_id INTEGER,
    merge_commit_sha TEXT,
    squash_commit_sha TEXT,
    merged_commit_sha TEXT,
    merge_error TEXT,
    rebase_commit_sha TEXT,
    rebase_in_progress INTEGER NOT NULL DEFAULT 0,
    latest_diff_id INTEGER,
    milestone_id INTEGER,
    time_estimate INTEGER NOT NULL DEFAULT 0,
    total_time_spent INTEGER NOT NULL DEFAULT 0,
    prepared_at INTEGER,
    last_edited_at INTEGER,
    last_edited_by_id INTEGER,
    updated_by_id INTEGER,
    merged_by_id INTEGER,
    merged_at INTEGER,
    latest_closed_by_id INTEGER,
    latest_closed_at INTEGER,
    merge_ref_sha TEXT,
    first_contribution INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (target_project_id, iid)
);

CREATE TABLE merge_request_assignees (
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (merge_request_id, user_id)
);

CREATE TABLE merge_request_reviewers (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    state INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    UNIQUE (merge_request_id, user_id)
);

-- One row per diff version of an MR.
CREATE TABLE merge_request_diffs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    state TEXT NOT NULL,
    base_commit_sha TEXT,
    start_commit_sha TEXT,
    head_commit_sha TEXT,
    real_size TEXT,
    commits_count INTEGER NOT NULL DEFAULT 0,
    patch_id_sha TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE approvals (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (merge_request_id, user_id)
);

-- EE approval rules, mirroring GitLab's tables. Project rules are the defaults; MR rules are
-- copied from them when an MR is created and can be overridden per MR.
CREATE TABLE approval_project_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    approvals_required INTEGER NOT NULL DEFAULT 0,
    rule_type TEXT NOT NULL DEFAULT 'regular',   -- regular, any_approver, report_approver
    report_type TEXT,
    applies_to_all_protected_branches INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE approval_project_rules_users (
    rule_id INTEGER NOT NULL REFERENCES approval_project_rules (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, user_id)
);

CREATE TABLE approval_project_rules_groups (
    rule_id INTEGER NOT NULL REFERENCES approval_project_rules (id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES namespaces (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, group_id)
);

CREATE TABLE approval_project_rules_protected_branches (
    rule_id INTEGER NOT NULL REFERENCES approval_project_rules (id) ON DELETE CASCADE,
    protected_branch_id INTEGER NOT NULL REFERENCES protected_branches (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, protected_branch_id)
);

CREATE TABLE approval_merge_request_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    approvals_required INTEGER NOT NULL DEFAULT 0,
    rule_type TEXT NOT NULL DEFAULT 'regular',
    report_type TEXT,
    -- The project rule this one was copied from (`approval_merge_request_rule_sources`).
    approval_project_rule_id INTEGER,
    modified_from_project_rule INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE approval_merge_request_rules_users (
    rule_id INTEGER NOT NULL REFERENCES approval_merge_request_rules (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, user_id)
);

CREATE TABLE approval_merge_request_rules_groups (
    rule_id INTEGER NOT NULL REFERENCES approval_merge_request_rules (id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES namespaces (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, group_id)
);

CREATE TABLE labels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    color TEXT NOT NULL,
    description TEXT,
    project_id INTEGER,
    group_id INTEGER,
    priority INTEGER,
    archived INTEGER NOT NULL DEFAULT 0,
    lock_on_merge INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE label_links (
    label_id INTEGER NOT NULL REFERENCES labels (id) ON DELETE CASCADE,
    target_type TEXT NOT NULL,
    target_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (label_id, target_type, target_id)
);

CREATE TABLE milestones (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    iid INTEGER NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    project_id INTEGER,
    group_id INTEGER,
    state TEXT NOT NULL DEFAULT 'active',
    due_date TEXT,
    start_date TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    note TEXT NOT NULL,
    noteable_type TEXT NOT NULL,
    noteable_id INTEGER NOT NULL,
    project_id INTEGER,
    author_id INTEGER NOT NULL,
    system INTEGER NOT NULL DEFAULT 0,
    system_action TEXT,
    type TEXT,
    discussion_id TEXT NOT NULL,
    resolvable INTEGER NOT NULL DEFAULT 0,
    resolved_at INTEGER,
    resolved_by_id INTEGER,
    resolved_by_push INTEGER NOT NULL DEFAULT 0,
    position TEXT,
    original_position TEXT,
    change_position TEXT,
    line_code TEXT,
    commit_id TEXT,
    internal INTEGER NOT NULL DEFAULT 0,
    review_id INTEGER,
    commands_changes TEXT,
    updated_by_id INTEGER,
    last_edited_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX notes_noteable ON notes (noteable_type, noteable_id);

CREATE TABLE draft_notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    author_id INTEGER NOT NULL,
    note TEXT NOT NULL,
    discussion_id TEXT,
    resolve_discussion INTEGER NOT NULL DEFAULT 0,
    position TEXT,
    original_position TEXT,
    line_code TEXT,
    commit_id TEXT,
    internal INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE reviews (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL,
    author_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE suggestions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    note_id INTEGER NOT NULL REFERENCES notes (id) ON DELETE CASCADE,
    relative_order INTEGER NOT NULL,
    from_content TEXT NOT NULL,
    to_content TEXT NOT NULL,
    lines_above INTEGER NOT NULL DEFAULT 0,
    lines_below INTEGER NOT NULL DEFAULT 0,
    applied INTEGER NOT NULL DEFAULT 0,
    commit_id TEXT,
    outdated INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE award_emoji (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    user_id INTEGER NOT NULL,
    awardable_type TEXT NOT NULL,
    awardable_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (user_id, awardable_type, awardable_id, name)
);

CREATE TABLE resource_state_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER,
    state TEXT NOT NULL,
    source_commit TEXT,
    source_merge_request_id INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE resource_label_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER,
    label_id INTEGER,
    action TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE resource_milestone_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER,
    milestone_id INTEGER,
    action TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE subscriptions (
    user_id INTEGER NOT NULL,
    subscribable_type TEXT NOT NULL,
    subscribable_id INTEGER NOT NULL,
    subscribed INTEGER NOT NULL,
    PRIMARY KEY (user_id, subscribable_type, subscribable_id)
);

-- `User#*_count` values in `Rails.cache`: computed on first read, kept for 24 hours unless a
-- service invalidates them.
CREATE TABLE user_count_cache (
    user_id INTEGER NOT NULL,
    key TEXT NOT NULL,
    value INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, key)
);

-- Background jobs that failed (labrat's own diagnostics; the test harness asserts none).
CREATE TABLE job_failures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    error TEXT NOT NULL
);

CREATE TABLE protected_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE protected_tag_create_access_levels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    protected_tag_id INTEGER NOT NULL REFERENCES protected_tags (id) ON DELETE CASCADE,
    access_level INTEGER NOT NULL
);

-- `user_statuses`: one optional status per user.
CREATE TABLE user_statuses (
    user_id INTEGER PRIMARY KEY,
    emoji TEXT NOT NULL,
    message TEXT,
    availability TEXT NOT NULL DEFAULT 'not_set',
    clear_status_at INTEGER
);

CREATE TABLE todos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    author_id INTEGER NOT NULL,
    project_id INTEGER,
    target_type TEXT NOT NULL,
    target_id INTEGER NOT NULL,
    action INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending',
    note_id INTEGER,
    commit_id TEXT,
    snoozed_until INTEGER,
    body TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE ci_pipelines (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    iid INTEGER NOT NULL,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    sha TEXT NOT NULL,
    ref TEXT,
    tag INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'created',
    source TEXT NOT NULL DEFAULT 'external',
    user_id INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    duration INTEGER,
    queued_duration INTEGER,
    -- Set with `PUT .../metadata`.
    name TEXT,
    before_sha TEXT
);

CREATE TABLE commit_statuses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pipeline_id INTEGER NOT NULL REFERENCES ci_pipelines (id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    ref TEXT,
    sha TEXT NOT NULL,
    status TEXT NOT NULL,
    target_url TEXT,
    description TEXT,
    coverage REAL,
    user_id INTEGER,
    retried INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    queued_at INTEGER,
    started_at INTEGER,
    finished_at INTEGER
);

-- `head_pipeline_id` and `merge_request_metrics` columns of GitLab's MR tables.
CREATE TABLE merge_request_pipelines (
    merge_request_id INTEGER PRIMARY KEY REFERENCES merge_requests (id) ON DELETE CASCADE,
    head_pipeline_id INTEGER,
    metrics_pipeline_id INTEGER,
    latest_build_started_at INTEGER,
    latest_build_finished_at INTEGER
);

CREATE TABLE merge_request_context_commits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    sha TEXT NOT NULL,
    relative_order INTEGER NOT NULL
);

CREATE TABLE merge_request_merge_schedules (
    merge_request_id INTEGER PRIMARY KEY REFERENCES merge_requests (id) ON DELETE CASCADE,
    merge_after INTEGER NOT NULL
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER,
    author_id INTEGER NOT NULL,
    action TEXT NOT NULL,
    target_type TEXT,
    target_id INTEGER,
    push_data TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE web_hooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER,
    type TEXT NOT NULL,              -- 'ProjectHook' or 'SystemHook'
    url TEXT NOT NULL,
    name TEXT,
    description TEXT,
    token TEXT,
    events TEXT NOT NULL DEFAULT '{}',
    push_events_branch_filter TEXT,
    branch_filter_strategy TEXT NOT NULL DEFAULT 'wildcard',
    enable_ssl_verification INTEGER NOT NULL DEFAULT 1,
    custom_headers TEXT NOT NULL DEFAULT '[]',
    custom_webhook_template TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE timelogs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    merge_request_id INTEGER NOT NULL REFERENCES merge_requests (id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL,
    time_spent INTEGER NOT NULL,
    summary TEXT,
    spent_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    -- The note whose `/spend` added it.
    note_id INTEGER
);

CREATE TABLE uploads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id INTEGER NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    secret TEXT NOT NULL,
    filename TEXT NOT NULL,
    data BLOB NOT NULL,
    uploaded_by_id INTEGER,
    created_at INTEGER NOT NULL
);

CREATE TABLE web_hook_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    web_hook_id INTEGER NOT NULL REFERENCES web_hooks (id) ON DELETE CASCADE,
    trigger TEXT NOT NULL,
    url TEXT NOT NULL,
    request_headers TEXT NOT NULL,
    request_data TEXT NOT NULL,
    response_headers TEXT,
    response_body TEXT,
    response_status TEXT,
    execution_duration REAL,
    internal_error_message TEXT,
    created_at INTEGER NOT NULL
);

-- Background jobs (labrat's stand-in for Sidekiq).
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    run_at INTEGER NOT NULL,
    running INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE application_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    settings TEXT NOT NULL DEFAULT '{}'
);
INSERT INTO application_settings (id) VALUES (1);

-- The cached to-do count: new to-dos write the count of all pending ones, snoozed included
-- (`UpdateTodoCountCacheService`); resolving, restoring or deleting clears it, and the next read
-- counts with `TodosFinder`, which leaves snoozed ones out. Snoozing touches neither.
CREATE TRIGGER todo_count_insert AFTER INSERT ON todos BEGIN
    INSERT OR REPLACE INTO user_count_cache (user_id, key, value, expires_at)
    VALUES (NEW.user_id, 'todos_pending_count',
            (SELECT COUNT(*) FROM todos WHERE user_id = NEW.user_id AND state = 'pending'),
            NEW.created_at + 86400000);
END;
CREATE TRIGGER todo_count_update AFTER UPDATE OF state ON todos BEGIN
    DELETE FROM user_count_cache WHERE user_id = NEW.user_id AND key = 'todos_pending_count';
END;
CREATE TRIGGER todo_count_delete AFTER DELETE ON todos BEGIN
    DELETE FROM user_count_cache WHERE user_id = OLD.user_id AND key = 'todos_pending_count';
END;

"#;
