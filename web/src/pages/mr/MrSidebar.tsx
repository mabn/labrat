import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api, enc } from '../../api/client';
import type { Label, MergeRequest, Milestone, MrBlock, Project, UserBasic } from '../../api/types';
import { CopyButton, Dropdown, LabelPill, MrStateBadge, UserAvatar, useToast, errorText } from '../../components/ui';
import { EeBadge, EeOnly, useEdition } from '../../edition';
import { localPath } from '../../lib/paths';
import { MultiUserSelect } from '../../components/UserSelect';
import { Icon } from '../../components/Icon';
import { useSession } from '../../session';
import { mrBase, useMrRefresh, useProjectUsers } from './data';

interface ReviewerState extends UserBasic {
  review_state?: string;
}

const reviewStateIcon: Record<string, [string, string, string]> = {
  approved: ['approve', 'var(--green-text)', 'Approved'],
  requested_changes: ['close', 'var(--red-500)', 'Requested changes'],
  reviewed: ['comment', 'var(--blue-500)', 'Reviewed'],
  unreviewed: ['eye', 'var(--gray-400)', 'Awaiting review'],
  review_started: ['history', 'var(--gray-500)', 'Reviewing'],
};

export function MrSidebar({ project, mr, canEdit }: { project: Project; mr: MergeRequest; canEdit: boolean }) {
  const { user } = useSession();
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const users = useProjectUsers(project);
  const labels = useQuery({ queryKey: ['labels', `/projects/${project.id}`], queryFn: () => api.all<Label>(`/projects/${project.id}/labels`, { include_ancestor_groups: true }) });
  const milestones = useQuery({ queryKey: ['milestones', project.id, 'active'], queryFn: () => api.get<Milestone[]>(`/projects/${project.id}/milestones`, { state: 'active', per_page: 100, include_ancestors: true }) });
  const reviewers = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'reviewers'],
    queryFn: async () => (await api.get<{ user: UserBasic; state: string }[]>(`${base}/reviewers`)).map((r) => ({ ...r.user, review_state: r.state }) as ReviewerState),
  });
  const update = useMutation({
    mutationFn: (body: Record<string, unknown>) => api.put(base, body),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const subscribe = useMutation({
    mutationFn: () => api.post(`${base}/${mr.subscribed ? 'unsubscribe' : 'subscribe'}`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const todo = useMutation({
    mutationFn: () => api.post(`${base}/todo`),
    onSuccess: () => {
      toast('To-do item added');
      refresh();
    },
    onError: (e) => toast(errorText(e), true),
  });
  const candidates = users.data ?? [];
  const labelMap = new Map((labels.data ?? []).map((l) => [l.name, l]));
  const toggleIds = (list: UserBasic[], u: UserBasic) => {
    const ids = list.map((x) => x.id);
    return ids.includes(u.id) ? ids.filter((i) => i !== u.id) : [...ids, u.id];
  };
  const time = mr.time_stats;
  return (
    <aside className="mr-sidebar">
      {user && (
        <div className="sidebar-block row">
          <button className="btn btn-sm" onClick={() => todo.mutate()}>
            <Icon name="todo" size={14} /> Add a to-do item
          </button>
          <span className="spacer" />
          <label className="row small" title="Notifications">
            <input type="checkbox" checked={!!mr.subscribed} onChange={() => subscribe.mutate()} /> Notifications
          </label>
        </div>
      )}
      <div className="sidebar-block">
        <div className="sidebar-block-head">
          {mr.assignees.length || 'No'} Assignee{mr.assignees.length === 1 ? '' : 's'}
          {canEdit && (
            <Dropdown
              right
              button={(_, toggle) => (
                <button className="btn btn-tertiary btn-sm" onClick={toggle}>
                  Edit
                </button>
              )}
            >
              {(close) => <MultiUserSelect candidates={candidates} selected={mr.assignees.map((a) => a.id)} close={close} onToggle={(u) => update.mutate({ assignee_ids: toggleIds(mr.assignees, u) })} />}
            </Dropdown>
          )}
        </div>
        {mr.assignees.length ? (
          mr.assignees.map((u) => (
            <div key={u.id} className="sidebar-user">
              <UserAvatar user={u} size={24} /> <span>{u.name}</span>
            </div>
          ))
        ) : (
          <div className="value">
            None
            {canEdit && user && (
              <>
                {' '}
                -{' '}
                <a href="#" onClick={(e) => { e.preventDefault(); update.mutate({ assignee_ids: [user.id] }); }}>
                  assign yourself
                </a>
              </>
            )}
          </div>
        )}
      </div>
      <div className="sidebar-block">
        <div className="sidebar-block-head">
          {mr.reviewers.length || 'No'} Reviewer{mr.reviewers.length === 1 ? '' : 's'}
          {canEdit && (
            <Dropdown
              right
              button={(_, toggle) => (
                <button className="btn btn-tertiary btn-sm" onClick={toggle}>
                  Edit
                </button>
              )}
            >
              {(close) => <MultiUserSelect candidates={candidates} selected={mr.reviewers.map((a) => a.id)} close={close} onToggle={(u) => update.mutate({ reviewer_ids: toggleIds(mr.reviewers, u) })} />}
            </Dropdown>
          )}
        </div>
        {(reviewers.data ?? mr.reviewers).length ? (
          (reviewers.data ?? mr.reviewers).map((u: ReviewerState) => {
            const st = reviewStateIcon[u.review_state ?? 'unreviewed'] ?? reviewStateIcon.unreviewed;
            return (
              <div key={u.id} className="sidebar-user">
                <UserAvatar user={u} size={24} /> <span className="grow">{u.name}</span>
                <span title={st[2]} style={{ color: st[1], display: 'inline-flex' }}>
                  <Icon name={st[0]} />
                </span>
              </div>
            );
          })
        ) : (
          <div className="value">
            None
            {canEdit && user && user.id !== mr.author.id && (
              <>
                {' '}
                -{' '}
                <a href="#" onClick={(e) => { e.preventDefault(); update.mutate({ reviewer_ids: [user.id] }); }}>
                  assign yourself
                </a>
              </>
            )}
          </div>
        )}
      </div>
      <div className="sidebar-block">
        <div className="sidebar-block-head">
          Labels
          {canEdit && (
            <Dropdown
              right
              button={(_, toggle) => (
                <button className="btn btn-tertiary btn-sm" onClick={toggle}>
                  Edit
                </button>
              )}
            >
              {() => (
                <>
                  {(labels.data ?? []).map((l) => {
                    const on = mr.labels.includes(l.name);
                    return (
                      <button key={l.id} className={`dropdown-item${on ? ' selected' : ''}`} onClick={() => update.mutate(on ? { remove_labels: l.name } : { add_labels: l.name })}>
                        <span style={{ width: 16 }}>{on && <Icon name="check" />}</span>
                        <span style={{ width: 12, height: 12, borderRadius: 3, background: l.color, display: 'inline-block' }} /> {l.name}
                      </button>
                    );
                  })}
                  {!labels.data?.length && <div className="dropdown-header">No labels. Create them on the Labels page.</div>}
                </>
              )}
            </Dropdown>
          )}
        </div>
        {mr.labels.length ? (
          <div className="row wrap" style={{ gap: 4 }}>
            {mr.labels.map((l) => (
              <LabelPill key={l} label={labelMap.get(l)} name={l} />
            ))}
          </div>
        ) : (
          <div className="value">None</div>
        )}
      </div>
      <div className="sidebar-block">
        <div className="sidebar-block-head">
          Milestone
          {canEdit && (
            <Dropdown
              right
              button={(_, toggle) => (
                <button className="btn btn-tertiary btn-sm" onClick={toggle}>
                  Edit
                </button>
              )}
            >
              {(close) => (
                <>
                  <button className="dropdown-item" onClick={() => { close(); update.mutate({ milestone_id: 0 }); }}>
                    No milestone
                  </button>
                  {(milestones.data ?? []).map((m) => (
                    <button key={m.id} className={`dropdown-item${mr.milestone?.id === m.id ? ' selected' : ''}`} onClick={() => { close(); update.mutate({ milestone_id: m.id }); }}>
                      {m.title}
                    </button>
                  ))}
                </>
              )}
            </Dropdown>
          )}
        </div>
        <div className="value">{mr.milestone?.title ?? 'None'}</div>
      </div>
      <Dependencies project={project} mr={mr} canEdit={canEdit} />
      <div className="sidebar-block">
        <div className="sidebar-block-head">Time tracking</div>
        <div className="value small">
          {time.total_time_spent || time.time_estimate ? (
            <>
              Spent: {time.human_total_time_spent ?? '0h'} · Estimate: {time.human_time_estimate ?? 'none'}
            </>
          ) : (
            <>No estimate or time spent. Use /estimate and /spend in a comment.</>
          )}
        </div>
      </div>
      <div className="sidebar-block">
        <div className="sidebar-block-head">
          Lock discussion
          {canEdit && (
            <button className="btn btn-tertiary btn-sm" onClick={() => update.mutate({ discussion_locked: !mr.discussion_locked })}>
              {mr.discussion_locked ? 'Unlock' : 'Lock'}
            </button>
          )}
        </div>
        <div className="value">{mr.discussion_locked ? 'Locked: only project members can comment.' : 'Unlocked'}</div>
      </div>
      <div className="sidebar-block">
        <div className="sidebar-block-head">Reference</div>
        <div className="row">
          <code className="grow truncate">{mr.references.full}</code>
          <CopyButton text={mr.references.full} title="Copy reference" />
        </div>
        <div className="row" style={{ marginTop: 6 }}>
          <span className="small subtle grow truncate">Source branch: {mr.source_branch}</span>
          <CopyButton text={mr.source_branch} title="Copy branch name" />
        </div>
      </div>
    </aside>
  );
}

/** `!1`, `project!1`, `group/project!1` or an MR URL, as the MR's project and iid. */
async function resolveReference(project: Project, raw: string): Promise<{ projectId: number; iid: number } | null> {
  const ref = raw.trim().replace(/\/$/, '');
  const url = ref.match(/^https?:\/\/[^/]+\/(.+?)\/-\/merge_requests\/(\d+)$/);
  const short = ref.match(/^(.*)!(\d+)$/);
  const [path, iid] = url ? [url[1], url[2]] : short ? [short[1], short[2]] : [null, null];
  if (path === null || !iid) return null;
  if (!path) return { projectId: project.id, iid: Number(iid) };
  const full = path.includes('/') ? path : `${project.namespace.full_path}/${path}`;
  const p = await api.get<Project>(`/projects/${enc(full)}`, { license: false });
  return { projectId: p.id, iid: Number(iid) };
}

function MrLine({ mr }: { mr: MergeRequest }) {
  return (
    <div className="row small" style={{ marginBottom: 4 }}>
      <MrStateBadge mr={mr} />
      <Link to={localPath(mr.web_url)} className="grow truncate" title={mr.title}>
        {mr.references.full}
      </Link>
    </div>
  );
}

/** EE merge request dependencies: what this MR waits for, and what waits for it. */
function Dependencies({ project, mr, canEdit }: { project: Project; mr: MergeRequest; canEdit: boolean }) {
  const { ee } = useEdition();
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const [adding, setAdding] = useState(false);
  const [ref, setRef] = useState('');
  const blocks = useQuery({
    queryKey: ['mr', project.id, mr.iid, 'blocks'],
    queryFn: async () => ({ blocks: await api.get<MrBlock[]>(`${base}/blocks`), blockees: await api.get<MrBlock[]>(`${base}/blockees`) }),
    enabled: ee === true,
  });
  const add = useMutation({
    mutationFn: async () => {
      const target = await resolveReference(project, ref);
      if (!target) throw new Error('Enter a reference such as !1, project!1, group/project!1 or a merge request URL.');
      await api.post(`${base}/blocks`, { blocking_merge_request_iid: target.iid, blocking_project_id: target.projectId });
    },
    onSuccess: () => {
      setRef('');
      setAdding(false);
      refresh();
    },
    onError: (e) => toast(errorText(e), true),
  });
  const remove = useMutation({
    mutationFn: (b: MrBlock) => api.del(`${base}/blocks/${b.id}`),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const list = blocks.data?.blocks ?? [];
  const blockees = (blocks.data?.blockees ?? []).filter((b) => b.blocked_merge_request);
  return (
    <div className="sidebar-block">
      <div className="sidebar-block-head">
        <span>
          Dependencies
          <EeBadge />
        </span>
        {canEdit && (
          <EeOnly note={false}>
            <button className="btn btn-tertiary btn-sm" onClick={() => setAdding((x) => !x)} title={ee === false ? 'Available in EE mode only' : undefined}>
              {adding ? 'Cancel' : 'Add'}
            </button>
          </EeOnly>
        )}
      </div>
      {adding && (
        <form
          className="row"
          style={{ marginBottom: 8 }}
          onSubmit={(e) => {
            e.preventDefault();
            add.mutate();
          }}
        >
          <input type="text" autoFocus placeholder="!1 or group/project!1" value={ref} onChange={(e) => setRef(e.target.value)} />
          <button className="btn btn-sm btn-confirm" disabled={!ref.trim() || add.isPending}>
            Add
          </button>
        </form>
      )}
      {ee === false ? (
        <div className="value subtle small">Available in EE mode only.</div>
      ) : list.length ? (
        <>
          <div className="small subtle" style={{ marginBottom: 4 }}>
            Blocked by
          </div>
          {list.map((b) =>
            b.blocking_merge_request ? (
              <div key={b.id} className="row">
                <div className="grow" style={{ minWidth: 0 }}>
                  <MrLine mr={b.blocking_merge_request} />
                </div>
                {canEdit && (
                  <button className="btn btn-tertiary btn-sm btn-icon" title="Remove" disabled={remove.isPending} onClick={() => remove.mutate(b)}>
                    <Icon name="close" size={12} />
                  </button>
                )}
              </div>
            ) : null,
          )}
          {list.some((b) => !b.blocking_merge_request) && <div className="small subtle">and merge requests you can't see</div>}
        </>
      ) : (
        <div className="value">None</div>
      )}
      {blockees.length > 0 && (
        <>
          <div className="small subtle" style={{ margin: '6px 0 4px' }}>
            Blocks
          </div>
          {blockees.map((b) => (
            <MrLine key={b.id} mr={b.blocked_merge_request!} />
          ))}
        </>
      )}
    </div>
  );
}
