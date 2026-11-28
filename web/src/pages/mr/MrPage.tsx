import { useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Commit, Discussion, MergeRequest, Pipeline, Project } from '../../api/types';
import { PipelineTable, RandomPipelineButton } from '../project/Pipelines';
import { CopyButton, Dropdown, Empty, ErrorBox, Loading, Markdown, MrStateBadge, TimeAgo, UserAvatar, useToast, errorText } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useSession } from '../../session';
import { useProject, useProjectCrumbs } from '../project/ProjectRoutes';
import { CommitList } from '../project/Commits';
import { useRefs } from '../project/refs';
import { mrBase, useApprovals, useDiscussions, useDrafts, useMr, useMrAwards, useMrRefresh, useNoteAwards } from './data';
import { MergeWidget } from './MergeWidget';
import { MrSidebar } from './MrSidebar';
import { Awards, CommentForm, SystemNote, Thread } from './Discussions';
import { Changes } from './Changes';
import { ReviewBar } from './ReviewBar';
import { mrUrl } from '../../lib/paths';

export function MrPage({ iid, tab }: { iid: number; tab: string }) {
  const { project, canWrite } = useProject();
  const { user } = useSession();
  const q = useMr(project, iid);
  const approvals = useApprovals(project, iid);
  const discussions = useDiscussions(project, iid);
  const drafts = useDrafts(project, iid, !!user);
  const noteAwards = useNoteAwards(project, iid);
  useProjectCrumbs([{ label: 'Merge requests', to: `/${project.path_with_namespace}/-/merge_requests` }, { label: `!${iid}` }]);
  if (q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  const mr = q.data!;
  const canEdit = canWrite || user?.id === mr.author.id;
  const userNotes = (discussions.data ?? []).reduce((n, d) => n + d.notes.filter((x) => !x.system).length, 0);
  const base = `/${project.path_with_namespace}/-/merge_requests/${iid}`;
  const tabs: [string, string, React.ReactNode][] = [
    ['', 'Overview', userNotes],
    ['commits', 'Commits', null],
    ['pipelines', 'Pipelines', null],
    ['diffs', 'Changes', mr.changes_count ?? null],
  ];
  return (
    <>
      <MrHeader project={project} mr={mr} canEdit={canEdit} />
      <div className="tabs" style={{ position: 'sticky', top: 48, background: '#fff', zIndex: 10 }}>
        {tabs.map(([t, label, count]) => (
          <Link key={t} to={t ? `${base}/${t}` : base} className={`tab${tab === t ? ' active' : ''}`}>
            {label}
            {count !== null && count !== undefined && <span className="count">{count}</span>}
          </Link>
        ))}
      </div>
      {tab === 'diffs' ? (
        <Changes project={project} mr={mr} discussions={discussions.data ?? []} drafts={drafts.data ?? []} noteAwards={noteAwards.data} />
      ) : (
        <div className="mr-layout">
          <div className="mr-main">
            {tab === 'commits' ? (
              <MrCommits project={project} mr={mr} />
            ) : tab === 'pipelines' ? (
              <MrPipelines project={project} mr={mr} />
            ) : (
              <MrOverview project={project} mr={mr} discussions={discussions.data} discussionsError={discussions.error} approvals={approvals.data} noteAwards={noteAwards.data} />
            )}
          </div>
          <MrSidebar project={project} mr={mr} canEdit={canEdit} />
        </div>
      )}
      <ReviewBar project={project} mr={mr} drafts={drafts.data ?? []} approvals={approvals.data} />
    </>
  );
}

function MrHeader({ project, mr, canEdit }: { project: Project; mr: MergeRequest; canEdit: boolean }) {
  const [editing, setEditing] = useState(false);
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const base = mrBase(project.id, mr.iid);
  const update = useMutation({
    mutationFn: (body: Record<string, unknown>) => api.put(base, body),
    onSuccess: refresh,
    onError: (e) => toast(errorText(e), true),
  });
  const toggleDraft = () => update.mutate({ title: mr.draft ? mr.title.replace(/^\s*(\[draft\]|\(draft\)|draft:|draft)\s*/i, '') : `Draft: ${mr.title}` });
  if (editing) return <EditMr project={project} mr={mr} onDone={() => setEditing(false)} />;
  return (
    <div style={{ paddingTop: 8 }}>
      <div className="row" style={{ alignItems: 'flex-start' }}>
        <h1 className="mr-title grow">{mr.title}</h1>
        {canEdit && (
          <div className="row" style={{ marginTop: 10 }}>
            <button className="btn" onClick={() => setEditing(true)}>
              Edit
            </button>
            <CodeDropdown project={project} mr={mr} />
            {mr.state !== 'merged' && (
              <Dropdown
                right
                button={(_, toggle) => (
                  <button className="btn btn-icon" onClick={toggle} title="Merge request actions">
                    <Icon name="ellipsis" />
                  </button>
                )}
              >
                {(close) => (
                  <>
                    {mr.state === 'opened' && (
                      <button className="dropdown-item" onClick={() => { close(); toggleDraft(); }}>
                        {mr.draft ? 'Mark as ready' : 'Mark as draft'}
                      </button>
                    )}
                    <button className="dropdown-item" onClick={() => { close(); update.mutate({ state_event: mr.state === 'closed' ? 'reopen' : 'close' }); }}>
                      {mr.state === 'closed' ? 'Reopen merge request' : 'Close merge request'}
                    </button>
                  </>
                )}
              </Dropdown>
            )}
          </div>
        )}
      </div>
      <div className="mr-meta">
        <MrStateBadge mr={mr} />
        {mr.draft && <span className="badge badge-neutral">Draft</span>}
        <span>
          <strong style={{ color: 'var(--text)' }}>{mr.author.name}</strong> requested to merge
        </span>
        <span className="branch-ref">{mr.source_branch}</span>
        <CopyButton text={mr.source_branch} title="Copy branch name" />
        <span>into</span>
        <span className="branch-ref">{mr.target_branch}</span>
        <span>
          <TimeAgo at={mr.created_at} />
        </span>
      </div>
    </div>
  );
}

function CodeDropdown({ project, mr }: { project: Project; mr: MergeRequest }) {
  const cmd = `git fetch origin\ngit checkout -b '${mr.source_branch}' 'origin/${mr.source_branch}'`;
  return (
    <Dropdown
      right
      button={(_, toggle) => (
        <button className="btn" onClick={toggle}>
          Code <Icon name="chevronDown" />
        </button>
      )}
    >
      {() => (
        <div style={{ padding: 12, width: 380 }}>
          <div className="label-text">Check out branch</div>
          <pre style={{ background: 'var(--gray-10)', padding: 8, border: '1px solid var(--border)', borderRadius: 4, whiteSpace: 'pre-wrap', fontSize: 12 }}>{cmd}</pre>
          <div className="row">
            <CopyButton text={cmd} title="Copy commands" />
            <a className="btn btn-sm" href={`/api/v4/projects/${project.id}/merge_requests/${mr.iid}/raw_diffs`} target="_blank" rel="noreferrer">
              Plain diff
            </a>
          </div>
        </div>
      )}
    </Dropdown>
  );
}

function EditMr({ project, mr, onDone }: { project: Project; mr: MergeRequest; onDone: () => void }) {
  const refresh = useMrRefresh(project, mr.iid);
  const refs = useRefs(project.id);
  const [title, setTitle] = useState(mr.title);
  const [description, setDescription] = useState(mr.description ?? '');
  const [target, setTarget] = useState(mr.target_branch);
  const [removeSource, setRemoveSource] = useState(!!mr.force_remove_source_branch);
  const [squash, setSquash] = useState(mr.squash);
  const [preview, setPreview] = useState(false);
  const save = useMutation({
    mutationFn: () => api.put(mrBase(project.id, mr.iid), { title, description, target_branch: target, remove_source_branch: removeSource, squash }),
    onSuccess: () => {
      refresh();
      onDone();
    },
  });
  return (
    <div className="card" style={{ margin: '12px 0' }}>
      <div className="card-head">Edit merge request {mr.references.short}</div>
      <div className="card-body">
        <ErrorBox error={save.error} />
        <label className="field">
          <span className="label">Title</span>
          <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} />
          <div className="help">Start the title with “Draft:” to prevent a merge request draft from merging before it’s ready.</div>
        </label>
        <div className="field">
          <div className="row">
            <span className="label-text grow">Description</span>
            <div className="btn-group">
              <button className={`btn btn-sm${!preview ? ' btn-confirm' : ''}`} onClick={() => setPreview(false)}>
                Write
              </button>
              <button className={`btn btn-sm${preview ? ' btn-confirm' : ''}`} onClick={() => setPreview(true)}>
                Preview
              </button>
            </div>
          </div>
          {preview ? (
            <div className="card-body" style={{ border: '1px solid var(--border)', borderRadius: 4, minHeight: 120 }}>
              <Markdown source={description} projectPath={project.path_with_namespace} />
            </div>
          ) : (
            <textarea rows={10} value={description} onChange={(e) => setDescription(e.target.value)} />
          )}
        </div>
        <label className="field">
          <span className="label">Target branch</span>
          <select value={target} onChange={(e) => setTarget(e.target.value)}>
            {refs.data?.branches.map((b) => (
              <option key={b.name} value={b.name}>
                {b.name}
              </option>
            ))}
          </select>
        </label>
        <label className="check">
          <input type="checkbox" checked={removeSource} onChange={(e) => setRemoveSource(e.target.checked)} /> Delete source branch when merge request is accepted.
        </label>
        {project.squash_option !== 'never' && project.squash_option !== 'always' && (
          <label className="check">
            <input type="checkbox" checked={squash} onChange={(e) => setSquash(e.target.checked)} /> Squash commits when merge request is accepted.
          </label>
        )}
        <div className="row" style={{ marginTop: 12 }}>
          <button className="btn btn-confirm" disabled={save.isPending || !title} onClick={() => save.mutate()}>
            Save changes
          </button>
          <button className="btn" onClick={onDone}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}

type Filter = 'all' | 'comments' | 'history';

function MrOverview({
  project,
  mr,
  discussions,
  discussionsError,
  approvals,
  noteAwards,
}: {
  project: Project;
  mr: MergeRequest;
  discussions?: Discussion[];
  discussionsError: unknown;
  approvals?: Parameters<typeof MergeWidget>[0]['approvals'];
  noteAwards?: Map<number, { name: string; username: string }[]>;
}) {
  const { user } = useSession();
  const refresh = useMrRefresh(project, mr.iid);
  const toast = useToast();
  const awards = useMrAwards(project, mr.iid);
  const [filter, setFilter] = useState<Filter>('all');
  const [sortDesc, setSortDesc] = useState(false);
  const base = mrBase(project.id, mr.iid);
  const toggleAward = async (name: string) => {
    try {
      const mine = awards.data?.find((a) => a.name === name && a.user.username === user?.username);
      if (mine) await api.del(`${base}/award_emoji/${mine.id}`);
      else await api.post(`${base}/award_emoji`, { name });
      refresh();
    } catch (e) {
      toast(errorText(e), true);
    }
  };
  const list = useMemo(() => {
    const l = (discussions ?? []).filter((d) => {
      const system = d.notes[0]?.system;
      return filter === 'all' || (filter === 'comments' ? !system : system);
    });
    return sortDesc ? [...l].reverse() : l;
  }, [discussions, filter, sortDesc]);
  const ctx = { project, mr, noteAwards };
  const resolvable = (discussions ?? []).filter((d) => d.resolvable);
  const resolved = resolvable.filter((d) => d.resolved).length;
  return (
    <>
      <div style={{ padding: '4px 0 16px', borderBottom: '1px solid var(--border)' }}>
        {mr.description ? <Markdown source={mr.description} projectPath={project.path_with_namespace} /> : <p className="subtle">No description provided.</p>}
        <Awards awards={(awards.data ?? []).map((a) => ({ name: a.name, username: a.user.username }))} onToggle={toggleAward} />
      </div>
      <MergeWidget project={project} mr={mr} approvals={approvals} />
      <div className="row" style={{ margin: '24px 0 12px' }}>
        <h2 className="grow" style={{ fontSize: '1.1rem' }}>
          Activity
        </h2>
        {resolvable.length > 0 && (
          <span className="badge badge-muted" title="Resolved threads">
            <Icon name="resolve" size={12} /> {resolved}/{resolvable.length} threads resolved
          </span>
        )}
        <select style={{ width: 'auto' }} value={sortDesc ? 'desc' : 'asc'} onChange={(e) => setSortDesc(e.target.value === 'desc')}>
          <option value="asc">Oldest first</option>
          <option value="desc">Newest first</option>
        </select>
        <select style={{ width: 'auto' }} value={filter} onChange={(e) => setFilter(e.target.value as Filter)}>
          <option value="all">All activity</option>
          <option value="comments">Comments only</option>
          <option value="history">History only</option>
        </select>
      </div>
      <ErrorBox error={discussionsError} />
      {!discussions ? (
        <Loading />
      ) : (
        <ul className="timeline">
          {list.map((d) => {
            const first = d.notes[0];
            if (first.system) {
              return (
                <li key={d.id} className="timeline-entry">
                  <SystemNote note={first} projectPath={project.path_with_namespace} />
                </li>
              );
            }
            const pos = first.type === 'DiffNote' ? first.position : null;
            return (
              <li key={d.id} className="timeline-entry">
                <Thread
                  discussion={d}
                  ctx={ctx}
                  collapsedByDefault={!!pos}
                  header={
                    pos ? (
                      <div className="discussion-diff-head row">
                        <Icon name="file" size={14} />
                        <Link to={mrUrl(project.path_with_namespace, mr.iid, 'diffs')} className="grow truncate">
                          {pos.new_path}
                          {pos.new_line ? `:${pos.new_line}` : pos.old_line ? `:${pos.old_line} (removed)` : ''}
                        </Link>
                        {pos.head_sha !== mr.diff_refs?.head_sha && <span className="badge badge-muted">outdated version</span>}
                      </div>
                    ) : undefined
                  }
                />
              </li>
            );
          })}
        </ul>
      )}
      {user && (
        <div className="note-card" style={{ marginTop: 16 }}>
          <UserAvatar user={user} size={32} />
          <div className="grow">
            {mr.discussion_locked && <div className="alert alert-warning">The discussion is locked. Only project members can comment.</div>}
            <CommentForm
              projectPath={project.path_with_namespace}
              actions={[
                { label: 'Comment', run: (body) => api.post(`${base}/notes`, { body }).then(refresh) },
                { label: 'Start thread', kind: 'default', run: (body) => api.post(`${base}/discussions`, { body }).then(refresh) },
                { label: 'Add to review', kind: 'default', run: (body) => api.post(`${base}/draft_notes`, { note: body }).then(refresh) },
              ]}
            />
          </div>
        </div>
      )}
    </>
  );
}

function MrCommits({ project, mr }: { project: Project; mr: MergeRequest }) {
  const q = useQuery({ queryKey: ['mr', project.id, mr.iid, 'commits', mr.sha], queryFn: () => api.all<Commit>(`${mrBase(project.id, mr.iid)}/commits`) });
  if (q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  if (!q.data?.length) return <Empty title="No commits" />;
  return <CommitList commits={q.data} projectPath={project.path_with_namespace} />;
}

function MrPipelines({ project, mr }: { project: Project; mr: MergeRequest }) {
  const q = useQuery({ queryKey: ['mr', project.id, mr.iid, 'pipelines'], queryFn: () => api.get<Pipeline[]>(`${mrBase(project.id, mr.iid)}/pipelines`), refetchInterval: 5000 });
  const head = (
    <div style={{ display: 'flex', justifyContent: 'flex-end', marginBottom: 8 }}>
      <RandomPipelineButton project={project} mr={mr} />
    </div>
  );
  if (q.isLoading) return <Loading />;
  if (q.error) return <ErrorBox error={q.error} />;
  if (!q.data?.length)
    return (
      <>
        {head}
        <Empty title="There are no pipelines for this merge request">
          <p>
            labrat runs no CI. Pipelines come from labrat&apos;s test endpoints (POST /-/labrat/projects/:id/pipelines, or the button above), the fake CI, and commit
            statuses (POST /projects/:id/statuses/:sha).
          </p>
        </Empty>
      </>
    );
  return (
    <>
      {head}
      <PipelineTable project={project} pipelines={q.data} />
    </>
  );
}
