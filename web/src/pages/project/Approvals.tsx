// EE merge request approvals in the project's merge request settings: approval rules and
// approval settings. Disabled in CE mode.

import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { ApprovalRule, GroupBasic, Project, ProjectApprovalSettings, ProtectedBranch, UserBasic } from '../../api/types';
import { Dropdown, ErrorBox, Loading, UserAvatar, errorText, useToast } from '../../components/ui';
import { MultiUserSelect } from '../../components/UserSelect';
import { Icon } from '../../components/Icon';
import { EeOnly, useEdition } from '../../edition';
import { useProjectUsers } from '../mr/data';

interface RuleForm {
  id?: number;
  name: string;
  approvals_required: number;
  users: UserBasic[];
  groups: GroupBasic[];
  /** '' for all branches, 'protected' for all protected branches, or a protected branch id. */
  branch: string;
}

const emptyForm: RuleForm = { name: '', approvals_required: 1, users: [], groups: [], branch: '' };

function branchLabel(r: ApprovalRule) {
  if (r.applies_to_all_protected_branches) return 'All protected branches';
  if (r.protected_branches?.length) return r.protected_branches.map((b) => b.name).join(', ');
  return 'All branches';
}

function RuleEditor({ project, initial, onDone }: { project: Project; initial: RuleForm; onDone: () => void }) {
  const qc = useQueryClient();
  const [f, setF] = useState(initial);
  const users = useProjectUsers(project);
  const groups = useQuery({ queryKey: ['groups-all'], queryFn: () => api.all<GroupBasic>('/groups', {}, 5), staleTime: 60_000 });
  const branches = useQuery({ queryKey: ['protected', project.id], queryFn: () => api.get<ProtectedBranch[]>(`/projects/${project.id}/protected_branches`, { per_page: 100 }) });
  const save = useMutation({
    mutationFn: () => {
      const any = !f.users.length && !f.groups.length;
      const body = {
        name: f.name,
        approvals_required: f.approvals_required,
        user_ids: f.users.map((u) => u.id),
        group_ids: f.groups.map((g) => g.id),
        applies_to_all_protected_branches: f.branch === 'protected',
        protected_branch_ids: f.branch && f.branch !== 'protected' ? [Number(f.branch)] : [],
        ...(f.id ? {} : { rule_type: any ? 'any_approver' : 'regular' }),
      };
      const base = `/projects/${project.id}/approval_rules`;
      return f.id ? api.put(`${base}/${f.id}`, body) : api.post(base, body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['approval-rules', project.id] });
      onDone();
    },
  });
  const toggle = <T extends { id: number }>(list: T[], x: T) => (list.some((y) => y.id === x.id) ? list.filter((y) => y.id !== x.id) : [...list, x]);
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <div className="card-head">{f.id ? 'Edit approval rule' : 'Add approval rule'}</div>
      <div className="card-body">
        <ErrorBox error={save.error} />
        <div className="row wrap" style={{ alignItems: 'flex-end' }}>
          <label>
            <div className="label-text">Rule name</div>
            <input type="text" value={f.name} placeholder="Examples: QA, Security" onChange={(e) => setF({ ...f, name: e.target.value })} style={{ width: 200 }} />
          </label>
          <label>
            <div className="label-text">Target branch</div>
            <select value={f.branch} onChange={(e) => setF({ ...f, branch: e.target.value })} style={{ width: 200 }}>
              <option value="">All branches</option>
              <option value="protected">All protected branches</option>
              {branches.data?.map((b) => (
                <option key={b.id} value={String(b.id)}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <div className="label-text">Required approvals</div>
            <input type="number" min={0} value={f.approvals_required} onChange={(e) => setF({ ...f, approvals_required: Number(e.target.value) })} style={{ width: 90 }} />
          </label>
        </div>
        <div className="row wrap" style={{ marginTop: 12 }}>
          <Dropdown
            button={(_, open) => (
              <button type="button" className="btn" onClick={open}>
                <Icon name="users" /> Users ({f.users.length}) <Icon name="chevronDown" />
              </button>
            )}
          >
            {(close) => <MultiUserSelect candidates={users.data ?? []} selected={f.users.map((u) => u.id)} close={close} onToggle={(u) => setF((x) => ({ ...x, users: toggle(x.users, u) }))} />}
          </Dropdown>
          <Dropdown
            button={(_, open) => (
              <button type="button" className="btn" onClick={open}>
                <Icon name="group" /> Groups ({f.groups.length}) <Icon name="chevronDown" />
              </button>
            )}
          >
            {() => (
              <>
                {(groups.data ?? []).map((g) => {
                  const on = f.groups.some((x) => x.id === g.id);
                  return (
                    <button type="button" key={g.id} className={`dropdown-item${on ? ' selected' : ''}`} onClick={() => setF((x) => ({ ...x, groups: toggle(x.groups, g) }))}>
                      <span style={{ width: 16 }}>{on && <Icon name="check" />}</span> {g.full_path}
                    </button>
                  );
                })}
                {!groups.data?.length && <div className="dropdown-header">No groups</div>}
              </>
            )}
          </Dropdown>
          <span className="small subtle">
            {f.users.length || f.groups.length
              ? [...f.users.map((u) => `@${u.username}`), ...f.groups.map((g) => g.full_path)].join(', ')
              : f.id
                ? 'No approvers'
                : 'Without approvers, any eligible user can approve.'}
          </span>
        </div>
        <div className="row" style={{ marginTop: 16 }}>
          <button type="button" className="btn btn-confirm" disabled={!f.name || save.isPending} onClick={() => save.mutate()}>
            Save changes
          </button>
          <button type="button" className="btn" onClick={onDone}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

function ApprovalRules({ project }: { project: Project }) {
  const { ee } = useEdition();
  const qc = useQueryClient();
  const toast = useToast();
  const [editing, setEditing] = useState<RuleForm | null>(null);
  const rules = useQuery({
    queryKey: ['approval-rules', project.id],
    queryFn: () => api.get<ApprovalRule[]>(`/projects/${project.id}/approval_rules`, { per_page: 100 }),
    enabled: ee === true,
  });
  const remove = useMutation({
    mutationFn: (r: ApprovalRule) => api.del(`/projects/${project.id}/approval_rules/${r.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['approval-rules', project.id] }),
    onError: (e) => toast(errorText(e), true),
  });
  const edit = (r: ApprovalRule) =>
    setEditing({
      id: r.id,
      name: r.name,
      approvals_required: r.approvals_required,
      users: r.users,
      groups: r.groups,
      branch: r.applies_to_all_protected_branches ? 'protected' : r.protected_branches?.length ? String(r.protected_branches[0].id) : '',
    });
  return (
    <>
      <div className="row" style={{ marginBottom: 12 }}>
        <h3 className="grow" style={{ fontSize: '1rem' }}>
          Approval rules
        </h3>
        {!editing && (
          <button type="button" className="btn btn-sm" onClick={() => setEditing(emptyForm)}>
            <Icon name="plus" size={14} /> Add approval rule
          </button>
        )}
      </div>
      {editing && <RuleEditor key={editing.id ?? 'new'} project={project} initial={editing} onDone={() => setEditing(null)} />}
      <ErrorBox error={rules.error} />
      {rules.isLoading ? (
        <Loading />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Rule</th>
                <th>Approvers</th>
                <th>Target branch</th>
                <th>Approvals required</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {(rules.data ?? []).map((r) => (
                <tr key={r.id}>
                  <td>{r.rule_type === 'any_approver' ? <span className="subtle">All eligible users</span> : r.name}</td>
                  <td>
                    <span className="avatar-stack">
                      {r.users.map((u) => (
                        <UserAvatar key={u.id} user={u} size={22} />
                      ))}
                    </span>{' '}
                    {r.groups.map((g) => g.full_path).join(', ')}
                  </td>
                  <td>{branchLabel(r)}</td>
                  <td>{r.approvals_required}</td>
                  <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                    <button type="button" className="btn btn-sm btn-tertiary" onClick={() => edit(r)} title="Edit">
                      <Icon name="edit" size={14} />
                    </button>
                    <button type="button" className="btn btn-sm btn-tertiary" onClick={() => remove.mutate(r)} title="Remove">
                      <Icon name="trash" size={14} />
                    </button>
                  </td>
                </tr>
              ))}
              {!rules.data?.length && (
                <tr>
                  <td colSpan={5} className="subtle">
                    No approval rules: approvals are optional.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}

type Locks = Record<string, { value: boolean; locked: boolean }>;

const defaults: ProjectApprovalSettings = {
  reset_approvals_on_push: true,
  selective_code_owner_removals: false,
  disable_overriding_approvers_per_merge_request: false,
  merge_requests_author_approval: false,
  merge_requests_disable_committers_approval: false,
  require_password_to_approve: false,
};

function ApprovalSettingsForm({ project }: { project: Project }) {
  const { ee } = useEdition();
  const qc = useQueryClient();
  const toast = useToast();
  const q = useQuery({
    queryKey: ['approval-settings', project.id],
    queryFn: async () => ({
      settings: await api.get<ProjectApprovalSettings>(`/projects/${project.id}/approvals`),
      locks: await api.get<Locks>(`/projects/${project.id}/merge_request_approval_setting`),
    }),
    enabled: ee === true,
  });
  const [f, setF] = useState<ProjectApprovalSettings>(defaults);
  useEffect(() => {
    // Only the settings this form edits: the GET also answers aliases (such as
    // `require_reauthentication_to_approve`) that would override them when sent back.
    if (q.data) setF(Object.fromEntries(Object.keys(defaults).map((k) => [k, q.data.settings[k as keyof ProjectApprovalSettings]])) as unknown as ProjectApprovalSettings);
  }, [q.data]);
  const save = useMutation({
    mutationFn: () => api.post(`/projects/${project.id}/approvals`, f),
    onSuccess: () => {
      toast('Approval settings saved');
      qc.invalidateQueries({ queryKey: ['approval-settings', project.id] });
    },
  });
  const locked = (k: string) => !!q.data?.locks[k]?.locked;
  const lockNote = (k: string) => (locked(k) ? <span className="subtle small"> (locked by the group)</span> : null);
  const check = (key: keyof ProjectApprovalSettings, label: string, lockKey: string, invert = false) => (
    <label className="check">
      <input type="checkbox" disabled={locked(lockKey)} checked={invert ? !f[key] : f[key]} onChange={(e) => setF({ ...f, [key]: invert ? !e.target.checked : e.target.checked })} />
      <span>
        {label}
        {lockNote(lockKey)}
      </span>
    </label>
  );
  // Selective code owner removals replace removing all approvals; the API refuses both at once.
  const onPush = f.selective_code_owner_removals ? 'code_owners' : f.reset_approvals_on_push ? 'all' : 'keep';
  return (
    <>
      <h3 style={{ fontSize: '1rem', margin: '24px 0 12px' }}>Approval settings</h3>
      <ErrorBox error={q.error || save.error} />
      {check('merge_requests_author_approval', 'Prevent approval by merge request creator', 'allow_author_approval', true)}
      {check('merge_requests_disable_committers_approval', 'Prevent approvals by users who add commits', 'allow_committer_approval')}
      {check('disable_overriding_approvers_per_merge_request', 'Prevent editing approval rules in merge requests', 'allow_overrides_to_approver_list_per_merge_request')}
      {check('require_password_to_approve', 'Require user password to approve', 'require_password_to_approve')}
      <div className="label-text" style={{ margin: '12px 0 6px' }}>
        When a commit is added:{lockNote('retain_approvals_on_push')}
      </div>
      {(
        [
          ['keep', 'Keep approvals'],
          ['all', 'Remove all approvals'],
          ['code_owners', 'Remove approvals by Code Owners if their files changed'],
        ] as const
      ).map(([v, label]) => (
        <label key={v} className="check">
          <input
            type="radio"
            disabled={locked('retain_approvals_on_push')}
            checked={onPush === v}
            onChange={() => setF({ ...f, reset_approvals_on_push: v === 'all', selective_code_owner_removals: v === 'code_owners' })}
          />
          <span>{label}</span>
        </label>
      ))}
      <button type="button" className="btn btn-confirm" style={{ marginTop: 8 }} disabled={save.isPending || !q.data} onClick={() => save.mutate()}>
        Save approval settings
      </button>
    </>
  );
}

/** The "Merge request approvals" section of the merge request settings. */
export function ProjectApprovals({ project }: { project: Project }) {
  return (
    <EeOnly>
      <p className="subtle" style={{ marginBottom: 12 }}>
        Set the number of necessary approvals and define a list of approvers needed to approve a merge request.
      </p>
      <ApprovalRules project={project} />
      <ApprovalSettingsForm project={project} />
    </EeOnly>
  );
}
