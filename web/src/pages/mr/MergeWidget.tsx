import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api, ApiError, enc } from '../../api/client';
import type { ApprovalRule, Approvals, MergeRequest, MergeTrainCar, Project, UserBasic } from '../../api/types';
import { CiStatus, ErrorBox, Loading, TimeAgo, UserAvatar, useToast, errorText } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useSession } from '../../session';
import { EeBadge, useEdition } from '../../edition';
import { mrBase, useMrRefresh } from './data';

const BLOCKED: Record<string, string> = {
  draft_status: 'Merge blocked: Select Mark as ready to remove it from Draft status.',
  discussions_not_resolved: 'Merge blocked: all threads must be resolved.',
  need_rebase: 'Merge blocked: the source branch must be rebased onto the target branch.',
  conflict: 'Merge blocked: merge conflicts must be resolved.',
  ci_must_pass: 'Merge blocked: pipeline must succeed. Push a commit that fixes the failure.',
  ci_still_running: 'Merge blocked: pipeline must succeed. It’s waiting for a manual action or still running.',
  not_approved: 'Merge blocked: all required approvals must be given.',
  requested_changes: 'Merge blocked: change requests must be approved by the requesting user.',
  blocked_status: 'Merge blocked: merge request dependencies must be merged.',
  not_open: 'Merge blocked: the merge request must be open.',
  broken_status: 'Merge blocked: the source branch is missing or has no changes.',
  commits_status: 'Merge blocked: the source branch exists and contains commits.',
  merge_time: 'Merge blocked: cannot be merged until the scheduled time.',
  merge_request_blocked: 'Merge blocked: this merge request is blocked by another one.',
  title_regex: 'Merge blocked: the title doesn’t match the required pattern.',
  locked_paths: 'Merge blocked: locked paths.',
  locked_lfs_files: 'Merge blocked: locked LFS files.',
  security_policy_violations: 'Merge blocked: security policy violations.',
  jira_association_missing: 'Merge blocked: a Jira issue key is required.',
};

function Section({ icon, children }: { icon: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="widget-section">
      <div className="widget-icon">{icon}</div>
      <div className="grow">{children}</div>
    </div>
  );
}

function StatusDot({ kind, icon }: { kind: string; icon: string }) {
  return (
    <span className={`status-icon ${kind}`}>
      <Icon name={icon} size={12} />
    </span>
  );
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

function ruleLabel(r: ApprovalRule) {
  if (r.rule_type === 'any_approver') return <span className="subtle">All eligible users</span>;
  return (
    <>
      {r.name}
      {r.section && r.section !== 'codeowners' && <span className="subtle small"> ({r.section})</span>}
      {r.rule_type === 'code_owner' && <span className="badge badge-neutral" style={{ marginLeft: 6 }}>Code owner</span>}
      {r.rule_type === 'report_approver' && <span className="badge badge-warning" style={{ marginLeft: 6 }}>Policy</span>}
    </>
  );
}

const Avatars = ({ users }: { users: UserBasic[] }) => (
  <span className="avatar-stack">
    {users.map((u) => (
      <UserAvatar key={u.id} user={u} size={20} />
    ))}
  </span>
);

/** EE: the MR's approval rules, from `/approval_state`. */
function ApprovalRulesTable({ project, mr }: { project: Project; mr: MergeRequest }) {
  const q = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'approval_state'],
    queryFn: () => api.get<{ rules: ApprovalRule[] }>(`${mrBase(project.id, mr.iid)}/approval_state`),
  });
  if (q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  const rules = q.data?.rules ?? [];
  if (!rules.length) return <div className="small subtle" style={{ marginTop: 8 }}>No approval rules.</div>;
  return (
    <div className="table-wrap" style={{ marginTop: 8 }}>
      <table className="table">
        <thead>
          <tr>
            <th>Approvers</th>
            <th>Approvals</th>
            <th>Eligible</th>
            <th>Approved by</th>
          </tr>
        </thead>
        <tbody>
          {rules.map((r) => (
            <tr key={r.id}>
              <td>{ruleLabel(r)}</td>
              <td>
                {r.approved ? <Icon name="check" size={14} /> : null} {r.approvals_required ? `${(r.approved_by ?? []).length} of ${r.approvals_required}` : 'Optional'}
              </td>
              <td>
                <Avatars users={r.eligible_approvers} />
              </td>
              <td>
                <Avatars users={r.approved_by ?? []} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ApprovalSection({ project, mr, approvals }: { project: Project; mr: MergeRequest; approvals?: Approvals }) {
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const { ee } = useEdition();
  const [showRules, setShowRules] = useState(false);
  const [password, setPassword] = useState('');
  const base = mrBase(project.id, mr.iid);
  const needsPassword = !!approvals?.require_password_to_approve;
  const m = useMutation({
    mutationFn: (approve: boolean) =>
      api.post(`${base}/${approve ? 'approve' : 'unapprove'}`, approve ? { sha: mr.sha, ...(needsPassword ? { approval_password: password } : {}) } : {}),
    onSuccess: () => {
      setPassword('');
      refresh();
    },
    onError: (e) => toast(errorText(e), true),
  });
  if (!approvals) return null;
  const by = approvals.approved_by;
  const left = approvals.approvals_left ?? 0;
  const rulesLeft = (approvals.approval_rules_left ?? []).filter((r) => r.rule_type !== 'any_approver').map((r) => r.name);
  const open = mr.state === 'opened';
  return (
    <Section icon={<StatusDot kind={left > 0 ? 'neutral' : by.length ? 'success' : 'neutral'} icon={left === 0 && by.length ? 'check' : 'approve'} />}>
      <div className="row wrap">
        {approvals.user_can_approve && open && (
          <>
            {needsPassword && (
              <input type="password" placeholder="Your password" value={password} onChange={(e) => setPassword(e.target.value)} style={{ width: 160 }} aria-label="Password to approve" />
            )}
            <button className="btn btn-confirm btn-sm" disabled={m.isPending || (needsPassword && !password)} onClick={() => m.mutate(true)}>
              Approve
            </button>
          </>
        )}
        {approvals.user_has_approved && open && (
          <button className="btn btn-sm" disabled={m.isPending} onClick={() => m.mutate(false)}>
            Revoke approval
          </button>
        )}
        {left > 0 && (
          <strong>
            Requires {plural(left, 'approval')}
            {rulesLeft.length ? ` from ${rulesLeft.join(', ')}` : ' from eligible users'}.
          </strong>
        )}
        {by.length ? (
          <>
            <strong>Approved by</strong>
            <Avatars users={by.map((a) => a.user)} />
            <span className="subtle small">{by.map((a) => a.user.name).join(', ')}</span>
          </>
        ) : (
          left === 0 && <span className="subtle">Approval is optional</span>
        )}
        <span className="spacer" />
        <button
          className="btn btn-tertiary btn-sm"
          disabled={ee !== true}
          title={ee === false ? 'Approval rules are available in EE mode only' : undefined}
          onClick={() => setShowRules((x) => !x)}
        >
          {showRules ? 'Hide' : 'Show'} approval rules <EeBadge />
        </button>
      </div>
      {showRules && ee && <ApprovalRulesTable project={project} mr={mr} />}
    </Section>
  );
}

interface PolicyData {
  project: {
    mergeRequest: {
      policyViolations: { policies: { name: string; status: string; dismissed: boolean; enforcementType: string; securityPolicyId: string; reportType: string }[] } | null;
      policyBypassStatuses: { id: string; name: string; allowBypass: boolean; bypassed: boolean }[] | null;
    } | null;
  } | null;
}

/** EE: approval policies this MR violates, with warn mode dismissals and bypasses. */
function PolicySection({ project, mr }: { project: Project; mr: MergeRequest }) {
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const vars = { path: project.path_with_namespace, iid: String(mr.iid) };
  const q = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'policies'],
    queryFn: () =>
      api.graphql<PolicyData>(
        `query($path: ID!, $iid: String!) { project(fullPath: $path) { mergeRequest(iid: $iid) {
          policyViolations { policies { name status dismissed enforcementType securityPolicyId reportType } }
          policyBypassStatuses { id name allowBypass bypassed }
        } } }`,
        vars,
      ),
    retry: false,
  });
  const act = useMutation({
    mutationFn: async ({ kind, id }: { kind: 'dismiss' | 'bypass'; id: string }) => {
      const text = prompt(kind === 'dismiss' ? 'Why is this violation dismissed?' : 'Why is this policy bypassed?');
      if (!text) return;
      const r = await api.graphql<Record<string, { errors: string[] }>>(
        kind === 'dismiss'
          ? `mutation($path: ID!, $iid: String!, $ids: [ID!]!, $text: String!) { r: dismissPolicyViolations(input: {projectPath: $path, iid: $iid, securityPolicyIds: $ids, dismissalTypes: [OTHER], comment: $text}) { errors } }`
          : `mutation($path: ID!, $iid: String!, $ids: [ID!]!, $text: String!) { r: mergeRequestBypassSecurityPolicy(input: {projectPath: $path, iid: $iid, securityPolicyIds: $ids, reason: $text}) { errors } }`,
        { ...vars, ids: [id], text },
      );
      if (r.r.errors.length) throw new Error(r.r.errors.join(', '));
    },
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const data = q.data?.project?.mergeRequest;
  const policies = data?.policyViolations?.policies ?? [];
  const bypass = new Map((data?.policyBypassStatuses ?? []).map((b) => [b.id, b]));
  if (!policies.length && !bypass.size) return null;
  const open = mr.state === 'opened';
  const failing = policies.filter((p) => p.status === 'FAILED' && !p.dismissed);
  return (
    <Section icon={<StatusDot kind={failing.length ? 'failed' : policies.length ? 'warning' : 'success'} icon={failing.length ? 'close' : 'lock'} />}>
      <div>
        <strong>{policies.length ? `Policy violations detected (${policies.length})` : 'Security policies'}</strong>
      </div>
      <ul className="small" style={{ margin: '6px 0 0', paddingLeft: 18 }}>
        {policies.map((p) => {
          const b = bypass.get(p.securityPolicyId);
          return (
            <li key={p.securityPolicyId} style={{ marginBottom: 4 }}>
              {p.name} <span className={`badge ${p.status === 'FAILED' ? 'badge-danger' : 'badge-warning'}`}>{p.status.toLowerCase()}</span>
              {p.enforcementType === 'WARN' && <span className="badge badge-neutral" style={{ marginLeft: 4 }}>warn mode</span>}
              {p.dismissed && <span className="badge badge-muted" style={{ marginLeft: 4 }}>dismissed</span>}
              {b?.bypassed && <span className="badge badge-muted" style={{ marginLeft: 4 }}>bypassed</span>}
              {open && p.enforcementType === 'WARN' && !p.dismissed && (
                <button className="btn btn-tertiary btn-sm" disabled={act.isPending} onClick={() => act.mutate({ kind: 'dismiss', id: p.securityPolicyId })}>
                  Dismiss
                </button>
              )}
              {open && b?.allowBypass && !b.bypassed && (
                <button className="btn btn-tertiary btn-sm" disabled={act.isPending} onClick={() => act.mutate({ kind: 'bypass', id: p.securityPolicyId })}>
                  Bypass
                </button>
              )}
            </li>
          );
        })}
      </ul>
    </Section>
  );
}

function MergedSection({ project, mr }: { project: Project; mr: MergeRequest }) {
  const toast = useToast();
  const refresh = useMrRefresh(project, mr.iid);
  const branch = useQuery({
    queryKey: ['branch-exists', project.id, mr.source_branch],
    queryFn: async () => {
      try {
        await api.get(`/projects/${project.id}/repository/branches/${enc(mr.source_branch)}`);
        return true;
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return false;
        throw e;
      }
    },
  });
  const del = useMutation({
    mutationFn: () => api.del(`/projects/${project.id}/repository/branches/${enc(mr.source_branch)}`),
    onSuccess: () => {
      toast('Source branch deleted');
      branch.refetch();
      refresh();
    },
    onError: (e) => toast(errorText(e), true),
  });
  const sha = mr.merge_commit_sha ?? mr.squash_commit_sha ?? mr.sha;
  return (
    <Section icon={<StatusDot kind="merged" icon="mr" />}>
      <div>
        <strong>Merged by</strong> {mr.merged_by?.name ?? mr.merge_user?.name} <TimeAgo at={mr.merged_at} />
      </div>
      <div className="small subtle" style={{ marginTop: 4 }}>
        Changes merged into <span className="branch-ref">{mr.target_branch}</span> with{' '}
        <Link to={`/${project.path_with_namespace}/-/commit/${sha}`}>
          <code>{sha?.slice(0, 8)}</code>
        </Link>
        . {branch.data === false && 'The source branch has been deleted.'}
      </div>
      {branch.data && (
        <button className="btn btn-sm" style={{ marginTop: 8 }} disabled={del.isPending} onClick={() => del.mutate()}>
          Delete source branch
        </button>
      )}
    </Section>
  );
}

function MergeSection({ project, mr }: { project: Project; mr: MergeRequest }) {
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const squashOpt = project.squash_option ?? 'default_off';
  const [squash, setSquash] = useState(squashOpt === 'always' || squashOpt === 'default_on' || mr.squash);
  const [removeSource, setRemoveSource] = useState(mr.force_remove_source_branch ?? project.remove_source_branch_after_merge ?? true);
  const [editMsg, setEditMsg] = useState(false);
  const [message, setMessage] = useState('');
  useEffect(() => {
    setRemoveSource(mr.force_remove_source_branch ?? project.remove_source_branch_after_merge ?? true);
  }, [mr.force_remove_source_branch, project.remove_source_branch_after_merge]);
  const merge = useMutation({
    mutationFn: (auto: boolean) =>
      api.put(`${base}/merge`, {
        sha: mr.sha,
        squash: squashOpt === 'never' ? undefined : squash,
        should_remove_source_branch: removeSource,
        auto_merge: auto || undefined,
        ...(editMsg && message ? (squash ? { squash_commit_message: message } : { merge_commit_message: message }) : {}),
      }),
    onSuccess: refresh,
  });
  const rebase = useMutation({
    mutationFn: () => api.put(`${base}/rebase`),
    onSuccess: () => {
      toast('Rebase started');
      refresh();
    },
    onError: (e) => toast(errorText(e), true),
  });
  const ready = useMutation({
    mutationFn: () => api.put(`${base}`, { title: mr.title.replace(/^\s*(\[draft\]|\(draft\)|draft:|draft)\s*/i, '') }),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const { ee } = useEdition();
  const trains = ee === true && !!project.merge_trains_enabled;
  const addToTrain = useMutation({
    mutationFn: (auto: boolean) =>
      api.post(`/projects/${project.id}/merge_trains/merge_requests/${mr.iid}`, { sha: mr.sha, squash: squashOpt === 'never' ? undefined : squash, auto_merge: auto || undefined }),
    onSuccess: refresh,
  });
  const skipTrain = useMutation({
    mutationFn: () => api.put(`${base}/merge`, { sha: mr.sha, squash: squashOpt === 'never' ? undefined : squash, should_remove_source_branch: removeSource, skip_merge_train: true }),
    onSuccess: refresh,
  });
  const car = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'train'],
    queryFn: async () => {
      let mine: MergeTrainCar;
      try {
        mine = await api.get<MergeTrainCar>(`/projects/${project.id}/merge_trains/merge_requests/${mr.iid}`);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
      const cars = await api.get<MergeTrainCar[]>(`/projects/${project.id}/merge_trains/${enc(mr.target_branch)}`, { scope: 'active', sort: 'asc', per_page: 100 });
      return { car: mine, ahead: Math.max(0, cars.findIndex((c) => c.id === mine.id)) };
    },
    enabled: trains && mr.merge_when_pipeline_succeeds,
    refetchInterval: 5000,
  });
  const cancelAuto = useMutation({
    mutationFn: () => api.post(`${base}/cancel_merge_when_pipeline_succeeds`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const status = mr.detailed_merge_status;
  const pipelineRunning = !!mr.head_pipeline && ['running', 'pending', 'created', 'waiting_for_resource', 'preparing', 'scheduled'].includes(mr.head_pipeline.status);
  const canMerge = mr.user?.can_merge;

  if (mr.merge_when_pipeline_succeeds && car.data) {
    const { car: c, ahead } = car.data;
    return (
      <Section icon={<StatusDot kind="info" icon="history" />}>
        <div>
          <strong>
            Added to the merge train by {c.user?.name ?? 'someone'}.{' '}
            {ahead === 0 ? 'It is first in line.' : `There ${ahead === 1 ? 'is 1 merge request' : `are ${ahead} merge requests`} ahead of it.`}
          </strong>
        </div>
        <div className="small subtle">
          {c.pipeline ? (
            <>
              Train pipeline #{c.pipeline.id}: {c.pipeline.status}.
            </>
          ) : (
            'Waiting for its pipeline.'
          )}{' '}
          <Link to={`/${project.path_with_namespace}/-/merge_trains`}>View merge trains</Link>
        </div>
        {canMerge && (
          <button className="btn btn-sm" style={{ marginTop: 8 }} disabled={cancelAuto.isPending} onClick={() => cancelAuto.mutate()}>
            Remove from merge train
          </button>
        )}
      </Section>
    );
  }
  if (mr.merge_when_pipeline_succeeds) {
    return (
      <Section icon={<StatusDot kind="info" icon="history" />}>
        <div>
          <strong>
            Set by {mr.merge_user?.name ?? 'someone'} to {trains ? 'be added to the merge train' : 'be merged automatically'} when all merge checks pass
          </strong>
        </div>
        <div className="small subtle">{mr.should_remove_source_branch || mr.force_remove_source_branch ? 'Deletes the source branch.' : 'Does not delete the source branch.'}</div>
        {canMerge && (
          <button className="btn btn-sm" style={{ marginTop: 8 }} disabled={cancelAuto.isPending} onClick={() => cancelAuto.mutate()}>
            Cancel auto-merge
          </button>
        )}
      </Section>
    );
  }
  if (['unchecked', 'checking', 'preparing', 'approvals_syncing'].includes(status) || mr.rebase_in_progress) {
    return (
      <Section icon={<Loading inline />}>
        {mr.rebase_in_progress ? 'Rebase in progress…' : 'Checking if merge request can be merged…'}
      </Section>
    );
  }
  const mergeable = status === 'mergeable';
  const autoMergeable = status === 'ci_still_running' || (mergeable && pipelineRunning);
  const behind = (mr.diverged_commits_count ?? 0) > 0;
  return (
    <Section icon={<StatusDot kind={mergeable ? 'success' : autoMergeable ? 'info' : 'failed'} icon={mergeable ? 'check' : autoMergeable ? 'history' : 'close'} />}>
      <ErrorBox error={merge.error || addToTrain.error || skipTrain.error} />
      {mr.merge_error && <div className="alert alert-danger">{mr.merge_error}</div>}
      {mergeable || autoMergeable ? (
        <>
          <div className="row wrap">
            {canMerge && trains ? (
              <>
                <button className="btn btn-confirm" disabled={addToTrain.isPending} onClick={() => addToTrain.mutate(autoMergeable)}>
                  {autoMergeable ? 'Set to auto-merge' : 'Add to merge train'}
                </button>
                {project.merge_trains_skip_train_allowed && !autoMergeable && (
                  <button className="btn btn-danger-secondary" disabled={skipTrain.isPending} onClick={() => confirm('Merge immediately, skipping the merge train?') && skipTrain.mutate()}>
                    Merge immediately
                  </button>
                )}
              </>
            ) : canMerge ? (
              autoMergeable ? (
                <button className="btn btn-confirm" disabled={merge.isPending} onClick={() => merge.mutate(true)}>
                  Set to auto-merge
                </button>
              ) : (
                <button className="btn btn-confirm" disabled={merge.isPending} onClick={() => merge.mutate(false)}>
                  {merge.isPending ? 'Merging…' : 'Merge'}
                </button>
              )
            ) : (
              <span className="subtle">Ready to merge by members who can write to the target branch.</span>
            )}
            {canMerge && <strong>{trains ? (autoMergeable ? 'Add to merge train when all checks pass' : 'Ready to merge!') : autoMergeable ? 'Merge when all checks pass' : 'Ready to merge!'}</strong>}
          </div>
          {canMerge && (
            <>
              <div className="merge-options">
                <label>
                  <input type="checkbox" checked={removeSource} onChange={(e) => setRemoveSource(e.target.checked)} disabled={mr.force_remove_source_branch === true && false} /> Delete source branch
                </label>
                {squashOpt !== 'never' && (
                  <label>
                    <input type="checkbox" checked={squash} disabled={squashOpt === 'always'} onChange={(e) => setSquash(e.target.checked)} /> Squash commits
                  </label>
                )}
                <label>
                  <input type="checkbox" checked={editMsg} onChange={(e) => setEditMsg(e.target.checked)} /> Edit commit message
                </label>
              </div>
              {editMsg && (
                <textarea
                  rows={4}
                  style={{ marginTop: 8 }}
                  className="mono"
                  placeholder={squash ? mr.title : `Merge branch '${mr.source_branch}' into '${mr.target_branch}'\n\n${mr.title}\n\nSee merge request ${mr.references.full}`}
                  value={message}
                  onChange={(e) => setMessage(e.target.value)}
                />
              )}
              <div className="small subtle" style={{ marginTop: 6 }}>
                {project.merge_method === 'ff' ? 'Fast-forward merge without a merge commit.' : project.merge_method === 'rebase_merge' ? 'Merge commit with semi-linear history.' : 'A merge commit will be added.'}
                {behind && ` The source branch is ${mr.diverged_commits_count} commit${mr.diverged_commits_count === 1 ? '' : 's'} behind the target branch.`}
              </div>
            </>
          )}
        </>
      ) : (
        <>
          <div>
            <strong>{BLOCKED[status] ?? `Merge blocked (${status}).`}</strong>
          </div>
          <div className="row wrap" style={{ marginTop: 8 }}>
            {status === 'draft_status' && canMerge && (
              <button className="btn btn-sm" disabled={ready.isPending} onClick={() => ready.mutate()}>
                Mark as ready
              </button>
            )}
            {(status === 'need_rebase' || (behind && project.merge_method !== 'merge')) && canMerge && (
              <button className="btn btn-sm" disabled={rebase.isPending} onClick={() => rebase.mutate()}>
                Rebase source branch
              </button>
            )}
            {status === 'conflict' && <span className="small subtle">Resolve the conflicts locally, then push to the source branch.</span>}
          </div>
        </>
      )}
      {(mergeable || autoMergeable) && behind && project.merge_method === 'merge' && canMerge && (
        <button className="btn btn-sm" style={{ marginTop: 8 }} disabled={rebase.isPending} onClick={() => rebase.mutate()}>
          Rebase
        </button>
      )}
    </Section>
  );
}

export function MergeWidget({ project, mr, approvals }: { project: Project; mr: MergeRequest; approvals?: Approvals }) {
  const { user } = useSession();
  const { ee } = useEdition();
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const reopen = useMutation({
    mutationFn: () => api.put(mrBase(project.id, mr.iid), { state_event: 'reopen' }),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <div className="widget">
      {mr.head_pipeline && (
        <Section icon={<CiStatus pipeline={mr.head_pipeline} />}>
          <div>
            Pipeline <strong>#{mr.head_pipeline.id}</strong> {mr.head_pipeline.status} for <code>{mr.head_pipeline.sha.slice(0, 8)}</code>
          </div>
        </Section>
      )}
      <ApprovalSection project={project} mr={mr} approvals={approvals} />
      {ee && <PolicySection project={project} mr={mr} />}
      {mr.state === 'merged' && <MergedSection project={project} mr={mr} />}
      {mr.state === 'closed' && (
        <Section icon={<StatusDot kind="failed" icon="close" />}>
          <div>
            <strong>Closed by</strong> {mr.closed_by?.name} <TimeAgo at={mr.closed_at} />
          </div>
          <div className="small subtle">The changes were not merged into {mr.target_branch}.</div>
          {user && (
            <button className="btn btn-sm" style={{ marginTop: 8 }} onClick={() => reopen.mutate()}>
              Reopen merge request
            </button>
          )}
        </Section>
      )}
      {mr.state === 'opened' && <MergeSection project={project} mr={mr} />}
      {mr.state === 'locked' && <Section icon={<Loading inline />}>Merging! Changes are being shipped…</Section>}
    </div>
  );
}
