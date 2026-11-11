import { useEffect, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { useMutation, useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Compare, Label, MergeRequest, Milestone, UserBasic } from '../../api/types';
import { Dropdown, Empty, ErrorBox, LabelPill, Loading, Markdown, UserAvatar } from '../../components/ui';
import { MultiUserSelect } from '../../components/UserSelect';
import { Icon } from '../../components/Icon';
import { useProject, useProjectCrumbs } from '../project/ProjectRoutes';
import { useRefs } from '../project/refs';
import { CommitList } from '../project/Commits';
import { DiffFile } from './DiffView';
import { useProjectUsers } from './data';
import { localPath } from '../../lib/paths';
import { useSession } from '../../session';

const humanize = (branch: string) => {
  const s = branch.replace(/^[^/]*\//, '').replace(/[-_]+/g, ' ').trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
};

export function NewMr() {
  const { project } = useProject();
  const { user } = useSession();
  const nav = useNavigate();
  const [params, setParams] = useSearchParams();
  useProjectCrumbs([{ label: 'Merge requests', to: `/${project.path_with_namespace}/-/merge_requests` }, { label: 'New' }]);
  const refs = useRefs(project.id);
  const source = params.get('source_branch') ?? params.get('merge_request[source_branch]') ?? '';
  const target = params.get('target_branch') ?? params.get('merge_request[target_branch]') ?? project.default_branch ?? 'main';
  const step2 = !!source && params.get('step') !== '1';
  const branches = refs.data?.branches ?? [];
  const [src, setSrc] = useState(source);
  const [tgt, setTgt] = useState(target);
  if (refs.isLoading) return <Loading />;
  if (!step2) {
    return (
      <>
        <div className="page-head">
          <h1>New merge request</h1>
        </div>
        <div className="row" style={{ alignItems: 'flex-start', gap: 24 }}>
          <div className="card grow">
            <div className="card-head">Source branch</div>
            <div className="card-body">
              <select value={src} onChange={(e) => setSrc(e.target.value)}>
                <option value="">Select source branch</option>
                {branches.map((b) => (
                  <option key={b.name} value={b.name}>
                    {b.name}
                  </option>
                ))}
              </select>
              {src && <div className="small subtle" style={{ marginTop: 8 }}>{branches.find((b) => b.name === src)?.commit.title}</div>}
            </div>
          </div>
          <div className="card grow">
            <div className="card-head">Target branch</div>
            <div className="card-body">
              <select value={tgt} onChange={(e) => setTgt(e.target.value)}>
                {branches.map((b) => (
                  <option key={b.name} value={b.name}>
                    {b.name}
                  </option>
                ))}
              </select>
              <div className="small subtle" style={{ marginTop: 8 }}>{branches.find((b) => b.name === tgt)?.commit.title}</div>
            </div>
          </div>
        </div>
        <button className="btn btn-confirm" style={{ marginTop: 16 }} disabled={!src || !tgt || src === tgt} onClick={() => setParams({ source_branch: src, target_branch: tgt })}>
          Compare branches and continue
        </button>
        {branches.length <= 1 && (
          <p className="subtle" style={{ marginTop: 16 }}>
            Push a branch, or create one from the <Link to={`/${project.path_with_namespace}/-/branches`}>Branches</Link> page or by editing a file, to open a merge request.
          </p>
        )}
      </>
    );
  }
  return <NewMrForm source={source} target={target} onChangeBranches={() => setParams({ source_branch: source, target_branch: target, step: '1' })} key={`${source}..${target}`} onCreated={(mr) => nav(localPath(mr.web_url))} userId={user?.id} />;
}

function NewMrForm({ source, target, onChangeBranches, onCreated, userId }: { source: string; target: string; onChangeBranches: () => void; onCreated: (mr: MergeRequest) => void; userId?: number }) {
  const { project } = useProject();
  const compare = useQuery({
    queryKey: ['compare', project.id, target, source],
    queryFn: () => api.get<Compare>(`/projects/${project.id}/repository/compare`, { from: target, to: source, straight: false }),
  });
  const users = useProjectUsers(project);
  const labels = useQuery({ queryKey: ['labels', `/projects/${project.id}`], queryFn: () => api.all<Label>(`/projects/${project.id}/labels`, { include_ancestor_groups: true }) });
  const milestones = useQuery({ queryKey: ['milestones', project.id, 'active'], queryFn: () => api.get<Milestone[]>(`/projects/${project.id}/milestones`, { state: 'active', per_page: 100, include_ancestors: true }) });
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState<string | null>(null);
  const [draft, setDraft] = useState(false);
  const [assignees, setAssignees] = useState<UserBasic[]>([]);
  const [reviewers, setReviewers] = useState<UserBasic[]>([]);
  const [chosenLabels, setLabels] = useState<string[]>([]);
  const [milestone, setMilestone] = useState<number | ''>('');
  const [removeSource, setRemoveSource] = useState(project.remove_source_branch_after_merge ?? true);
  const [squash, setSquash] = useState(project.squash_option === 'default_on' || project.squash_option === 'always');
  const [preview, setPreview] = useState(false);
  const [tab, setTab] = useState<'commits' | 'changes'>('commits');
  useEffect(() => {
    const commits = compare.data?.commits ?? [];
    if (!title && compare.data) setTitle(commits.length === 1 ? commits[0].title : humanize(source));
    if (description === null && compare.data) {
      const one = commits.length === 1 ? commits[0].message.slice(commits[0].title.length).trim() : '';
      setDescription(project.merge_requests_template ?? one);
    }
  }, [compare.data]); // eslint-disable-line react-hooks/exhaustive-deps
  const create = useMutation({
    mutationFn: () =>
      api.post<MergeRequest>(`/projects/${project.id}/merge_requests`, {
        source_branch: source,
        target_branch: target,
        title: draft && !/^draft:/i.test(title) ? `Draft: ${title}` : title,
        description: description || undefined,
        assignee_ids: assignees.map((u) => u.id),
        reviewer_ids: reviewers.map((u) => u.id),
        labels: chosenLabels.join(','),
        milestone_id: milestone || undefined,
        remove_source_branch: removeSource,
        squash: project.squash_option === 'never' ? undefined : squash,
      }),
    onSuccess: onCreated,
  });
  const toggle = (list: UserBasic[], set: (l: UserBasic[]) => void) => (u: UserBasic) => set(list.some((x) => x.id === u.id) ? list.filter((x) => x.id !== u.id) : [...list, u]);
  const candidates = users.data ?? [];
  return (
    <>
      <div className="page-head">
        <div>
          <h1>New merge request</h1>
          <div className="row subtle small" style={{ marginTop: 6 }}>
            From <span className="branch-ref">{source}</span> into <span className="branch-ref">{target}</span>
            <button className="btn btn-sm btn-tertiary" onClick={onChangeBranches}>
              Change branches
            </button>
          </div>
        </div>
      </div>
      <ErrorBox error={create.error || compare.error} />
      {compare.data && !compare.data.commits.length && <div className="alert alert-warning">There are no changes between {source} and {target}. labrat will still create the merge request, as GitLab does.</div>}
      <div className="mr-layout">
        <div className="mr-main">
          <label className="field">
            <span className="label">Title (required)</span>
            <input type="text" value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
          <label className="check">
            <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} />
            <span>
              Mark as draft
              <div className="help">Drafts cannot be merged until marked ready.</div>
            </span>
          </label>
          <div className="field">
            <div className="row" style={{ marginBottom: 6 }}>
              <span className="label-text grow" style={{ margin: 0 }}>
                Description
              </span>
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
              <div className="card-body" style={{ border: '1px solid var(--border)', borderRadius: 4, minHeight: 160 }}>
                <Markdown source={description ?? ''} projectPath={project.path_with_namespace} />
              </div>
            ) : (
              <textarea rows={8} value={description ?? ''} placeholder="Describe the goal of the changes and what reviewers should be aware of." onChange={(e) => setDescription(e.target.value)} />
            )}
          </div>
          <div className="field">
            <span className="label-text">Merge options</span>
            <label className="check">
              <input type="checkbox" checked={removeSource} onChange={(e) => setRemoveSource(e.target.checked)} /> Delete source branch when merge request is accepted.
            </label>
            {project.squash_option !== 'never' && (
              <label className="check">
                <input type="checkbox" checked={squash} disabled={project.squash_option === 'always'} onChange={(e) => setSquash(e.target.checked)} /> Squash commits when merge request is accepted.
              </label>
            )}
          </div>
          <div className="row">
            <button className="btn btn-confirm" disabled={!title || create.isPending} onClick={() => create.mutate()}>
              Create merge request
            </button>
            <Link className="btn" to={`/${project.path_with_namespace}/-/merge_requests`}>
              Cancel
            </Link>
          </div>
        </div>
        <aside className="mr-sidebar">
          <div className="sidebar-block">
            <div className="sidebar-block-head">
              Assignees
              <Dropdown right button={(_, t) => <button className="btn btn-tertiary btn-sm" onClick={t}>Edit</button>}>
                {(close) => <MultiUserSelect candidates={candidates} selected={assignees.map((u) => u.id)} close={close} onToggle={toggle(assignees, setAssignees)} />}
              </Dropdown>
            </div>
            {assignees.length ? (
              assignees.map((u) => (
                <div key={u.id} className="sidebar-user">
                  <UserAvatar user={u} /> {u.name}
                </div>
              ))
            ) : (
              <div className="value">
                Unassigned
                {userId && candidates.some((c) => c.id === userId) && (
                  <>
                    {' '}
                    -{' '}
                    <a href="#" onClick={(e) => { e.preventDefault(); setAssignees(candidates.filter((c) => c.id === userId)); }}>
                      assign yourself
                    </a>
                  </>
                )}
              </div>
            )}
          </div>
          <div className="sidebar-block">
            <div className="sidebar-block-head">
              Reviewers
              <Dropdown right button={(_, t) => <button className="btn btn-tertiary btn-sm" onClick={t}>Edit</button>}>
                {(close) => <MultiUserSelect candidates={candidates} selected={reviewers.map((u) => u.id)} close={close} onToggle={toggle(reviewers, setReviewers)} />}
              </Dropdown>
            </div>
            {reviewers.length ? (
              reviewers.map((u) => (
                <div key={u.id} className="sidebar-user">
                  <UserAvatar user={u} /> {u.name}
                </div>
              ))
            ) : (
              <div className="value">None</div>
            )}
          </div>
          <div className="sidebar-block">
            <div className="sidebar-block-head">
              Labels
              <Dropdown right button={(_, t) => <button className="btn btn-tertiary btn-sm" onClick={t}>Edit</button>}>
                {() => (
                  <>
                    {labels.data?.map((l) => {
                      const on = chosenLabels.includes(l.name);
                      return (
                        <button key={l.id} className="dropdown-item" onClick={() => setLabels(on ? chosenLabels.filter((x) => x !== l.name) : [...chosenLabels, l.name])}>
                          <span style={{ width: 16 }}>{on && <Icon name="check" />}</span> {l.name}
                        </button>
                      );
                    })}
                    {!labels.data?.length && <div className="dropdown-header">No labels</div>}
                  </>
                )}
              </Dropdown>
            </div>
            <div className="row wrap" style={{ gap: 4 }}>
              {chosenLabels.length ? chosenLabels.map((n) => <LabelPill key={n} label={labels.data?.find((l) => l.name === n)} name={n} />) : <span className="value">None</span>}
            </div>
          </div>
          <div className="sidebar-block">
            <div className="sidebar-block-head">Milestone</div>
            <select value={milestone} onChange={(e) => setMilestone(e.target.value ? Number(e.target.value) : '')}>
              <option value="">No milestone</option>
              {milestones.data?.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.title}
                </option>
              ))}
            </select>
          </div>
        </aside>
      </div>
      <div className="tabs" style={{ marginTop: 24 }}>
        <button className={`tab${tab === 'commits' ? ' active' : ''}`} onClick={() => setTab('commits')}>
          Commits <span className="count">{compare.data?.commits.length ?? '…'}</span>
        </button>
        <button className={`tab${tab === 'changes' ? ' active' : ''}`} onClick={() => setTab('changes')}>
          Changes <span className="count">{compare.data?.diffs.length ?? '…'}</span>
        </button>
      </div>
      {compare.isLoading ? (
        <Loading />
      ) : tab === 'commits' ? (
        compare.data?.commits.length ? <CommitList commits={[...compare.data.commits].reverse()} projectPath={project.path_with_namespace} /> : <Empty title="No commits" />
      ) : (
        compare.data?.diffs.map((f) => <DiffFile key={f.old_path + f.new_path} file={f} />)
      )}
    </>
  );
}
