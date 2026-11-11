import { useEffect, useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api, ApiError, enc } from '../../api/client';
import type { Approvals, MergeRequest, Project } from '../../api/types';
import { CiStatus, ErrorBox, Loading, TimeAgo, UserAvatar, useToast, errorText } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useSession } from '../../session';
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

function ApprovalSection({ project, mr, approvals }: { project: Project; mr: MergeRequest; approvals?: Approvals }) {
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const m = useMutation({
    mutationFn: (approve: boolean) => api.post(`${base}/${approve ? 'approve' : 'unapprove'}`, approve ? { sha: mr.sha } : {}),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  if (!approvals) return null;
  const by = approvals.approved_by;
  return (
    <Section icon={<StatusDot kind={by.length ? 'success' : 'neutral'} icon={by.length ? 'check' : 'approve'} />}>
      <div className="row wrap">
        {approvals.user_can_approve && mr.state === 'opened' && (
          <button className="btn btn-confirm btn-sm" disabled={m.isPending} onClick={() => m.mutate(true)}>
            Approve
          </button>
        )}
        {approvals.user_has_approved && mr.state === 'opened' && (
          <button className="btn btn-sm" disabled={m.isPending} onClick={() => m.mutate(false)}>
            Revoke approval
          </button>
        )}
        {by.length ? (
          <>
            <strong>Approved by</strong>
            <span className="avatar-stack">
              {by.map((a) => (
                <UserAvatar key={a.user.id} user={a.user} size={22} />
              ))}
            </span>
            <span className="subtle small">{by.map((a) => a.user.name).join(', ')}</span>
          </>
        ) : (
          <span className="subtle">Approval is optional</span>
        )}
      </div>
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
  const cancelAuto = useMutation({
    mutationFn: () => api.post(`${base}/cancel_merge_when_pipeline_succeeds`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const status = mr.detailed_merge_status;
  const pipelineRunning = !!mr.head_pipeline && ['running', 'pending', 'created', 'waiting_for_resource', 'preparing', 'scheduled'].includes(mr.head_pipeline.status);
  const canMerge = mr.user?.can_merge;

  if (mr.merge_when_pipeline_succeeds) {
    return (
      <Section icon={<StatusDot kind="info" icon="history" />}>
        <div>
          <strong>Set by {mr.merge_user?.name ?? 'someone'} to be merged automatically when all merge checks pass</strong>
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
      <ErrorBox error={merge.error} />
      {mr.merge_error && <div className="alert alert-danger">{mr.merge_error}</div>}
      {mergeable || autoMergeable ? (
        <>
          <div className="row wrap">
            {canMerge ? (
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
            {canMerge && <strong>{autoMergeable ? 'Merge when all checks pass' : 'Ready to merge!'}</strong>}
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
