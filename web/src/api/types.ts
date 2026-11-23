// The parts of GitLab's REST entities the UI uses.

export interface UserBasic {
  id: number;
  username: string;
  name: string;
  state: string;
  avatar_url: string | null;
  web_url: string;
}

export interface User extends UserBasic {
  email?: string;
  is_admin?: boolean;
  created_at?: string;
  bio?: string;
  bot?: boolean;
}

export interface Namespace {
  id: number;
  name: string;
  path: string;
  kind: 'user' | 'group';
  full_path: string;
  parent_id: number | null;
  web_url: string;
}

export interface Project {
  id: number;
  name: string;
  name_with_namespace: string;
  path: string;
  path_with_namespace: string;
  description: string | null;
  default_branch: string | null;
  visibility: 'private' | 'internal' | 'public';
  ssh_url_to_repo: string;
  http_url_to_repo: string;
  web_url: string;
  readme_url?: string | null;
  star_count: number;
  forks_count: number;
  last_activity_at: string;
  created_at: string;
  namespace: Namespace;
  empty_repo?: boolean;
  archived?: boolean;
  open_issues_count?: number;
  merge_method?: 'merge' | 'rebase_merge' | 'ff';
  squash_option?: 'never' | 'always' | 'default_on' | 'default_off';
  only_allow_merge_if_pipeline_succeeds?: boolean;
  only_allow_merge_if_all_discussions_are_resolved?: boolean;
  remove_source_branch_after_merge?: boolean;
  /** EE: merged results pipelines and merge trains. */
  merge_pipelines_enabled?: boolean;
  merge_trains_enabled?: boolean;
  merge_trains_skip_train_allowed?: boolean;
  merge_requests_enabled?: boolean;
  merge_commit_template?: string | null;
  squash_commit_template?: string | null;
  merge_requests_template?: string | null;
  permissions?: {
    project_access: { access_level: number } | null;
    group_access: { access_level: number } | null;
  };
  statistics?: Record<string, number>;
}

export interface Group {
  id: number;
  name: string;
  path: string;
  full_name: string;
  full_path: string;
  description: string;
  visibility: string;
  web_url: string;
  parent_id: number | null;
  avatar_url: string | null;
}

export interface Member extends UserBasic {
  access_level: number;
  expires_at: string | null;
}

export interface Commit {
  id: string;
  short_id: string;
  title: string;
  message: string;
  author_name: string;
  author_email: string;
  authored_date: string;
  committer_name: string;
  committed_date: string;
  created_at: string;
  parent_ids: string[];
  web_url: string;
  stats?: { additions: number; deletions: number; total: number };
  last_pipeline?: Pipeline | null;
}

export interface Branch {
  name: string;
  merged: boolean;
  protected: boolean;
  default: boolean;
  developers_can_push: boolean;
  developers_can_merge: boolean;
  can_push: boolean;
  web_url: string;
  commit: Commit;
}

export interface Tag {
  name: string;
  message: string | null;
  target: string;
  commit: Commit;
  protected: boolean;
}

export interface TreeEntry {
  id: string;
  name: string;
  type: 'tree' | 'blob' | 'commit';
  path: string;
  mode: string;
}

export interface RepoFile {
  file_name: string;
  file_path: string;
  size: number;
  encoding: string;
  content: string;
  ref: string;
  blob_id: string;
  commit_id: string;
  last_commit_id: string;
}

export interface Label {
  id: number;
  name: string;
  color: string;
  text_color: string;
  description: string | null;
  open_merge_requests_count?: number;
  priority?: number | null;
  is_project_label?: boolean;
}

export interface Milestone {
  id: number;
  iid: number;
  title: string;
  description: string | null;
  state: 'active' | 'closed';
  due_date: string | null;
  start_date: string | null;
  web_url: string;
  project_id?: number;
  group_id?: number;
}

export interface Pipeline {
  id: number;
  iid?: number;
  sha: string;
  ref: string;
  status: string;
  source?: string;
  web_url: string;
  created_at?: string;
  updated_at?: string;
}

export interface DiffRefs {
  base_sha: string;
  head_sha: string;
  start_sha: string;
}

export interface MergeRequest {
  id: number;
  iid: number;
  project_id: number;
  title: string;
  description: string | null;
  state: 'opened' | 'closed' | 'merged' | 'locked';
  created_at: string;
  updated_at: string;
  merged_at: string | null;
  closed_at: string | null;
  merged_by: UserBasic | null;
  merge_user: UserBasic | null;
  closed_by: UserBasic | null;
  target_branch: string;
  source_branch: string;
  source_project_id: number;
  target_project_id: number;
  user_notes_count: number;
  upvotes: number;
  downvotes: number;
  author: UserBasic;
  assignees: UserBasic[];
  reviewers: UserBasic[];
  labels: string[];
  milestone: Milestone | null;
  draft: boolean;
  work_in_progress: boolean;
  merge_when_pipeline_succeeds: boolean;
  merge_status: string;
  detailed_merge_status: string;
  sha: string;
  merge_commit_sha: string | null;
  squash_commit_sha: string | null;
  squash: boolean;
  squash_on_merge: boolean;
  should_remove_source_branch: boolean | null;
  force_remove_source_branch: boolean | null;
  discussion_locked: boolean | null;
  references: { short: string; relative: string; full: string };
  web_url: string;
  time_stats: { time_estimate: number; total_time_spent: number; human_time_estimate: string | null; human_total_time_spent: string | null };
  task_completion_status: { count: number; completed_count: number };
  has_conflicts: boolean;
  blocking_discussions_resolved: boolean;
  // Single-MR fields.
  changes_count?: string | null;
  diff_refs?: DiffRefs | null;
  head_pipeline?: Pipeline | null;
  pipeline?: Pipeline | null;
  user?: { can_merge: boolean };
  diverged_commits_count?: number;
  rebase_in_progress?: boolean;
  first_contribution?: boolean;
  merge_error?: string | null;
  subscribed?: boolean;
}

export interface Position {
  base_sha: string;
  start_sha: string;
  head_sha: string;
  old_path: string;
  new_path: string;
  position_type: 'text' | 'image' | 'file';
  old_line: number | null;
  new_line: number | null;
  line_range?: unknown;
}

export interface Suggestion {
  id: number;
  from_line: number;
  to_line: number;
  appliable: boolean;
  applied: boolean;
  from_content: string;
  to_content: string;
}

export interface Note {
  id: number;
  type: 'DiscussionNote' | 'DiffNote' | null;
  body: string;
  author: UserBasic;
  created_at: string;
  updated_at: string;
  system: boolean;
  noteable_id: number;
  noteable_type: string;
  resolvable: boolean;
  resolved?: boolean;
  resolved_by?: UserBasic | null;
  resolved_at?: string | null;
  position?: Position | null;
  suggestions?: Suggestion[];
  internal: boolean;
}

export interface Discussion {
  id: string;
  individual_note: boolean;
  resolvable: boolean;
  resolved?: boolean;
  notes: Note[];
}

export interface DraftNote {
  id: number;
  author_id: number;
  merge_request_id: number;
  resolve_discussion: boolean;
  discussion_id: string | null;
  note: string;
  commit_id: string | null;
  line_code: string | null;
  position: Position;
}

export interface Award {
  id: number;
  name: string;
  user: UserBasic;
  awardable_id: number;
  awardable_type: string;
}

export interface Approvals {
  user_has_approved: boolean;
  user_can_approve: boolean;
  approved: boolean;
  approved_by: { user: UserBasic; approved_at?: string }[];
  // EE only:
  approvals_required?: number;
  approvals_left?: number;
  require_password_to_approve?: boolean;
  approval_rules_left?: { id: number; name: string; rule_type: string }[];
}

export interface GroupBasic {
  id: number;
  name: string;
  full_path: string;
}

/** EE: a project's or a merge request's approval rule. */
export interface ApprovalRule {
  id: number;
  name: string;
  rule_type: string;
  report_type?: string | null;
  section?: string | null;
  approvals_required: number;
  eligible_approvers: UserBasic[];
  users: UserBasic[];
  groups: GroupBasic[];
  protected_branches?: { id: number; name: string }[];
  applies_to_all_protected_branches?: boolean;
  code_owner?: boolean;
  approved?: boolean;
  approved_by?: UserBasic[];
}

/** EE: `/projects/:id/approvals`. */
export interface ProjectApprovalSettings {
  reset_approvals_on_push: boolean;
  selective_code_owner_removals: boolean;
  disable_overriding_approvers_per_merge_request: boolean;
  merge_requests_author_approval: boolean;
  merge_requests_disable_committers_approval: boolean;
  require_password_to_approve: boolean;
}

/** EE: a merge train car. */
export interface MergeTrainCar {
  id: number;
  merge_request: { id: number; iid: number; project_id: number; title: string; state: string; web_url: string };
  user: UserBasic | null;
  pipeline: Pipeline | null;
  created_at: string;
  target_branch: string;
  status: string;
  merged_at: string | null;
  duration: number | null;
}

/** EE: a merge request dependency (`/blocks`). */
export interface MrBlock {
  id: number;
  blocking_merge_request?: MergeRequest;
  blocked_merge_request?: MergeRequest;
}

export interface FileDiff {
  old_path: string;
  new_path: string;
  a_mode: string;
  b_mode: string;
  new_file: boolean;
  renamed_file: boolean;
  deleted_file: boolean;
  generated_file?: boolean;
  too_large?: boolean;
  collapsed?: boolean;
  diff: string;
}

export interface MrVersion {
  id: number;
  head_commit_sha: string;
  base_commit_sha: string;
  start_commit_sha: string;
  created_at: string;
  merge_request_id: number;
  state: string;
  real_size: string;
}

export interface Hook {
  id: number;
  url: string;
  name?: string | null;
  description?: string | null;
  created_at: string;
  push_events: boolean;
  tag_push_events: boolean;
  merge_requests_events: boolean;
  note_events: boolean;
  emoji_events?: boolean;
  pipeline_events: boolean;
  enable_ssl_verification: boolean;
  push_events_branch_filter?: string | null;
  alert_status?: string;
}

export interface HookEvent {
  id: number;
  url: string;
  trigger: string;
  request_headers: Record<string, string>;
  request_data: unknown;
  response_headers: Record<string, string>;
  response_body: string;
  execution_duration: number;
  response_status: string | number;
  created_at?: string;
}

export interface ProtectedBranch {
  id: number;
  name: string;
  push_access_levels: { access_level: number; access_level_description: string }[];
  merge_access_levels: { access_level: number; access_level_description: string }[];
  allow_force_push: boolean;
  code_owner_approval_required?: boolean;
}

export interface Token {
  id: number;
  name: string;
  revoked: boolean;
  created_at: string;
  scopes: string[];
  user_id: number;
  last_used_at: string | null;
  active: boolean;
  expires_at: string | null;
  token?: string;
}

export interface Todo {
  id: number;
  project: Project;
  author: UserBasic;
  action_name: string;
  target_type: string;
  target: MergeRequest;
  target_url: string;
  body: string;
  state: string;
  created_at: string;
}

export interface Compare {
  commit: Commit | null;
  commits: Commit[];
  diffs: FileDiff[];
  compare_timeout: boolean;
  compare_same_ref: boolean;
  web_url: string;
}

export interface Event {
  id: number;
  project_id: number;
  action_name: string;
  target_id: number | null;
  target_iid: number | null;
  target_type: string | null;
  author: UserBasic;
  target_title: string | null;
  created_at: string;
  push_data?: { ref: string; commit_count: number; action: string; commit_title: string | null; ref_type: string };
  note?: Note;
}

export const ACCESS_LEVELS: Record<number, string> = {
  0: 'No access',
  5: 'Minimal Access',
  10: 'Guest',
  15: 'Planner',
  20: 'Reporter',
  30: 'Developer',
  40: 'Maintainer',
  50: 'Owner',
};
