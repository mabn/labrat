import { useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, enc } from '../../api/client';
import type { Project, ProtectedBranch, Token } from '../../api/types';
import { Empty, ErrorBox, Loading, useToast, errorText } from '../../components/ui';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { useRefs } from './refs';
import { TokenForm, TokenTable } from '../AccessTokens';
import { EeBadge, EeOnly, useEdition } from '../../edition';
import { ProjectApprovals } from './Approvals';

function useSaveProject() {
  const { project } = useProject();
  const qc = useQueryClient();
  const toast = useToast();
  return useMutation({
    mutationFn: (body: Partial<Project> & Record<string, unknown>) => api.put<Project>(`/projects/${project.id}`, body),
    onSuccess: () => {
      toast(`Project '${project.name}' was successfully updated.`);
      qc.invalidateQueries({ queryKey: ['project', project.id] });
      qc.invalidateQueries({ queryKey: ['resolve'] });
    },
  });
}

function Section({ title, help, ee, children }: { title: string; help?: string; ee?: boolean; children: ReactNode }) {
  return (
    <section style={{ padding: '16px 0 24px', borderBottom: '1px solid var(--border)' }}>
      <h2 style={{ fontSize: '1.15rem' }}>
        {title}
        {ee && <EeBadge />}
      </h2>
      {help && <p className="subtle" style={{ marginTop: 4 }}>{help}</p>}
      <div style={{ marginTop: 12 }}>{children}</div>
    </section>
  );
}

export function GeneralSettings() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Settings' }, { label: 'General' }]);
  const save = useSaveProject();
  const nav = useNavigate();
  const qc = useQueryClient();
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description ?? '');
  const [visibility, setVisibility] = useState(project.visibility);
  const [path, setPath] = useState(project.path);
  const archive = useMutation({
    mutationFn: () => api.post(`/projects/${project.id}/${project.archived ? 'unarchive' : 'archive'}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['project', project.id] }),
  });
  const rename = useMutation({
    mutationFn: () => api.put<Project>(`/projects/${project.id}`, { path }),
    onSuccess: (p) => {
      qc.invalidateQueries({ queryKey: ['resolve'] });
      window.location.href = `/${p.path_with_namespace}/-/settings/general`;
    },
  });
  const remove = useMutation({
    mutationFn: () => api.del(`/projects/${project.id}`),
    onSuccess: () => nav('/dashboard/projects'),
  });
  return (
    <>
      <div className="page-head">
        <h1>General settings</h1>
      </div>
      <Section title="Naming, description, visibility">
        <form
          className="form-narrow"
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate({ name, description, visibility });
          }}
        >
          <ErrorBox error={save.error} />
          <label className="field">
            <span className="label">Project name</span>
            <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="field">
            <span className="label">Project ID</span>
            <input type="text" readOnly value={project.id} />
          </label>
          <label className="field">
            <span className="label">Project description (optional)</span>
            <textarea rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
          </label>
          <label className="field">
            <span className="label">Project visibility</span>
            <select value={visibility} onChange={(e) => setVisibility(e.target.value as Project['visibility'])}>
              <option value="private">Private</option>
              <option value="internal">Internal</option>
              <option value="public">Public</option>
            </select>
          </label>
          <button className="btn btn-confirm" disabled={save.isPending}>
            Save changes
          </button>
        </form>
      </Section>
      <Section title="Change path" help="A project’s repository name defines its URL and its place on the file disk.">
        <div className="form-narrow">
          <ErrorBox error={rename.error} />
          <div className="input-group" style={{ marginBottom: 12 }}>
            <span>{location.origin}/{project.namespace.full_path}/</span>
            <input type="text" value={path} onChange={(e) => setPath(e.target.value)} />
          </div>
          <button className="btn btn-danger" disabled={path === project.path || rename.isPending} onClick={() => rename.mutate()}>
            Change path
          </button>
        </div>
      </Section>
      <Section title={project.archived ? 'Unarchive project' : 'Archive project'} help="Archiving the project makes it entirely read-only.">
        <ErrorBox error={archive.error} />
        <button className="btn btn-danger-secondary" onClick={() => archive.mutate()}>
          {project.archived ? 'Unarchive project' : 'Archive project'}
        </button>
      </Section>
      <Section title="Delete project" help="This deletes the project, its repository and all related resources, including merge requests.">
        <ErrorBox error={remove.error} />
        <button
          className="btn btn-danger"
          onClick={() => prompt(`Enter ${project.path_with_namespace} to confirm`) === project.path_with_namespace && remove.mutate()}
        >
          Delete project
        </button>
      </Section>
    </>
  );
}

export function MergeRequestSettings() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Settings' }, { label: 'Merge requests' }]);
  const save = useSaveProject();
  const { ee } = useEdition();
  const [trains, setTrains] = useState({
    merge_pipelines_enabled: !!project.merge_pipelines_enabled,
    merge_trains_enabled: !!project.merge_trains_enabled,
    merge_trains_skip_train_allowed: !!project.merge_trains_skip_train_allowed,
  });
  const [f, setF] = useState({
    merge_method: project.merge_method ?? 'merge',
    squash_option: project.squash_option ?? 'default_off',
    only_allow_merge_if_pipeline_succeeds: !!project.only_allow_merge_if_pipeline_succeeds,
    only_allow_merge_if_all_discussions_are_resolved: !!project.only_allow_merge_if_all_discussions_are_resolved,
    remove_source_branch_after_merge: !!project.remove_source_branch_after_merge,
    merge_commit_template: project.merge_commit_template ?? '',
    squash_commit_template: project.squash_commit_template ?? '',
    merge_requests_template: project.merge_requests_template ?? '',
  });
  const set = (k: keyof typeof f, v: unknown) => setF((x) => ({ ...x, [k]: v }));
  return (
    <>
      <div className="page-head">
        <h1>Merge requests</h1>
      </div>
      <form
        className="form-narrow"
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate({
            ...f,
            ...(ee ? trains : {}),
            merge_commit_template: f.merge_commit_template || null,
            squash_commit_template: f.squash_commit_template || null,
            merge_requests_template: f.merge_requests_template || null,
          } as Partial<Project>);
        }}
      >
        <ErrorBox error={save.error} />
        <Section title="Merge method" help="Determine what happens to the commit history when you merge a merge request.">
          {[
            ['merge', 'Merge commit', 'Every merge creates a merge commit.'],
            ['rebase_merge', 'Merge commit with semi-linear history', 'Every merge creates a merge commit. Merging is only allowed when the source branch is up to date with its target.'],
            ['ff', 'Fast-forward merge', 'No merge commits are created. Merging is only allowed when the source branch is up to date with its target.'],
          ].map(([v, label, help]) => (
            <label key={v} className="check">
              <input type="radio" checked={f.merge_method === v} onChange={() => set('merge_method', v)} />
              <span>
                <strong>{label}</strong>
                <div className="help">{help}</div>
              </span>
            </label>
          ))}
        </Section>
        <Section title="Merge options">
          <label className="check">
            <input type="checkbox" checked={f.remove_source_branch_after_merge} onChange={(e) => set('remove_source_branch_after_merge', e.target.checked)} />
            <span>Enable “Delete source branch” option by default</span>
          </label>
        </Section>
        <Section title="Merge trains" ee help="Merged results pipelines test the merged result; merge trains queue merges so each one is tested against the ones ahead of it.">
          <EeOnly>
            <label className="check">
              <input type="checkbox" checked={trains.merge_pipelines_enabled} onChange={(e) => setTrains({ ...trains, merge_pipelines_enabled: e.target.checked })} />
              <span>Enable merged results pipelines</span>
            </label>
            <label className="check">
              <input type="checkbox" checked={trains.merge_trains_enabled} disabled={!trains.merge_pipelines_enabled} onChange={(e) => setTrains({ ...trains, merge_trains_enabled: e.target.checked })} />
              <span>
                Enable merge trains
                <div className="help">Needs merged results pipelines.</div>
              </span>
            </label>
            <label className="check">
              <input type="checkbox" checked={trains.merge_trains_skip_train_allowed} disabled={!trains.merge_trains_enabled} onChange={(e) => setTrains({ ...trains, merge_trains_skip_train_allowed: e.target.checked })} />
              <span>Allow skipping the merge train</span>
            </label>
          </EeOnly>
        </Section>
        <Section title="Squash commits when merging">
          {[
            ['never', 'Do not allow', 'Squashing is never performed and the checkbox is hidden.'],
            ['default_off', 'Allow', 'Checkbox is visible and unselected by default.'],
            ['default_on', 'Encourage', 'Checkbox is visible and selected by default.'],
            ['always', 'Require', 'Squashing is always performed. Checkbox is visible and selected, and users cannot change it.'],
          ].map(([v, label, help]) => (
            <label key={v} className="check">
              <input type="radio" checked={f.squash_option === v} onChange={() => set('squash_option', v)} />
              <span>
                <strong>{label}</strong>
                <div className="help">{help}</div>
              </span>
            </label>
          ))}
        </Section>
        <Section title="Merge checks" help="These checks must pass before merge requests can be merged.">
          <label className="check">
            <input type="checkbox" checked={f.only_allow_merge_if_pipeline_succeeds} onChange={(e) => set('only_allow_merge_if_pipeline_succeeds', e.target.checked)} />
            <span>Pipelines must succeed</span>
          </label>
          <label className="check">
            <input type="checkbox" checked={f.only_allow_merge_if_all_discussions_are_resolved} onChange={(e) => set('only_allow_merge_if_all_discussions_are_resolved', e.target.checked)} />
            <span>All threads must be resolved</span>
          </label>
        </Section>
        <Section title="Templates">
          <label className="field">
            <span className="label">Merge commit message template</span>
            <textarea rows={3} className="mono" placeholder="Merge branch '%{source_branch}' into '%{target_branch}'" value={f.merge_commit_template} onChange={(e) => set('merge_commit_template', e.target.value)} />
          </label>
          <label className="field">
            <span className="label">Squash commit message template</span>
            <textarea rows={2} className="mono" placeholder="%{title}" value={f.squash_commit_template} onChange={(e) => set('squash_commit_template', e.target.value)} />
          </label>
          <label className="field">
            <span className="label">Default description template for merge requests</span>
            <textarea rows={4} value={f.merge_requests_template} onChange={(e) => set('merge_requests_template', e.target.value)} />
          </label>
        </Section>
        <div style={{ marginTop: 16 }}>
          <button className="btn btn-confirm" disabled={save.isPending}>
            Save changes
          </button>
        </div>
      </form>
      <Section title="Merge request approvals" ee>
        <ProjectApprovals project={project} />
      </Section>
    </>
  );
}

const LEVELS: [number, string][] = [
  [0, 'No one'],
  [30, 'Developers + Maintainers'],
  [40, 'Maintainers'],
];

export function RepositorySettings() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Settings' }, { label: 'Repository' }]);
  const refs = useRefs(project.id);
  const save = useSaveProject();
  const qc = useQueryClient();
  const toast = useToast();
  const [branch, setBranch] = useState(project.default_branch ?? '');
  const protectedQ = useQuery({ queryKey: ['protected', project.id], queryFn: () => api.get<ProtectedBranch[]>(`/projects/${project.id}/protected_branches`, { per_page: 100 }) });
  const [pName, setPName] = useState('');
  const [push, setPush] = useState(40);
  const [merge, setMerge] = useState(40);
  const [force, setForce] = useState(false);
  const [codeOwners, setCodeOwners] = useState(false);
  const { ee } = useEdition();
  const protect = useMutation({
    mutationFn: () =>
      api.post(`/projects/${project.id}/protected_branches`, {
        name: pName,
        push_access_level: push,
        merge_access_level: merge,
        allow_force_push: force,
        ...(ee ? { code_owner_approval_required: codeOwners } : {}),
      }),
    onSuccess: () => {
      setPName('');
      qc.invalidateQueries({ queryKey: ['protected', project.id] });
      qc.invalidateQueries({ queryKey: ['refs', project.id] });
    },
  });
  const unprotect = useMutation({
    mutationFn: (name: string) => api.del(`/projects/${project.id}/protected_branches/${enc(name)}`),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ['protected', project.id] });
      qc.invalidateQueries({ queryKey: ['refs', project.id] });
    },
    onError: (e) => toast(errorText(e), true),
  });
  const patchBranch = useMutation({
    mutationFn: ({ b, body }: { b: ProtectedBranch; body: Partial<ProtectedBranch> }) => api.patch(`/projects/${project.id}/protected_branches/${enc(b.name)}`, body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['protected', project.id] }),
    onError: (e) => toast(errorText(e), true),
  });
  return (
    <>
      <div className="page-head">
        <h1>Repository settings</h1>
      </div>
      <Section title="Branch defaults" help="Set the default branch for this project.">
        <div className="form-narrow">
          <ErrorBox error={save.error} />
          <label className="field">
            <span className="label">Default branch</span>
            <select value={branch} onChange={(e) => setBranch(e.target.value)}>
              {refs.data?.branches.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
          <button className="btn btn-confirm" disabled={save.isPending || branch === project.default_branch} onClick={() => save.mutate({ default_branch: branch })}>
            Save changes
          </button>
        </div>
      </Section>
      <Section title="Protected branches" help="Keep stable branches secure and force developers to use merge requests.">
        <div className="card" style={{ marginBottom: 16 }}>
          <div className="card-head">Protect a branch</div>
          <div className="card-body">
            <ErrorBox error={protect.error} />
            <div className="row wrap" style={{ alignItems: 'flex-end' }}>
              <label>
                <div className="label-text">Branch</div>
                <input type="text" list="branch-names" placeholder="Name or wildcard (release/*)" value={pName} onChange={(e) => setPName(e.target.value)} style={{ width: 220 }} />
                <datalist id="branch-names">
                  {refs.data?.branches.map((b) => (
                    <option key={b.name} value={b.name} />
                  ))}
                </datalist>
              </label>
              <label>
                <div className="label-text">Allowed to merge</div>
                <select value={merge} onChange={(e) => setMerge(Number(e.target.value))} style={{ width: 210 }}>
                  {LEVELS.map(([v, l]) => (
                    <option key={v} value={v}>
                      {l}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <div className="label-text">Allowed to push and merge</div>
                <select value={push} onChange={(e) => setPush(Number(e.target.value))} style={{ width: 210 }}>
                  {LEVELS.map(([v, l]) => (
                    <option key={v} value={v}>
                      {l}
                    </option>
                  ))}
                </select>
              </label>
              <label className="row" style={{ height: 32 }}>
                <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} /> Allow force push
              </label>
              <EeOnly note={false}>
                <label className="row" style={{ height: 32 }} title={ee === false ? 'Available in EE mode only' : undefined}>
                  <input type="checkbox" checked={codeOwners} onChange={(e) => setCodeOwners(e.target.checked)} /> Require approval from code owners
                  <EeBadge />
                </label>
              </EeOnly>
              <button className="btn btn-confirm" disabled={!pName || protect.isPending} onClick={() => protect.mutate()}>
                Protect
              </button>
            </div>
          </div>
        </div>
        {protectedQ.isLoading ? (
          <Loading />
        ) : protectedQ.data?.length ? (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Branch</th>
                  <th>Allowed to merge</th>
                  <th>Allowed to push and merge</th>
                  <th>Allowed to force push</th>
                  <th>
                    Code owner approval
                    <EeBadge />
                  </th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {protectedQ.data.map((b) => (
                  <tr key={b.id}>
                    <td className="mono">{b.name}</td>
                    <td>{b.merge_access_levels.map((l) => l.access_level_description).join(', ')}</td>
                    <td>{b.push_access_levels.map((l) => l.access_level_description).join(', ')}</td>
                    <td>
                      <input type="checkbox" checked={b.allow_force_push} onChange={() => patchBranch.mutate({ b, body: { allow_force_push: !b.allow_force_push } })} />
                    </td>
                    <td>
                      <input
                        type="checkbox"
                        disabled={ee !== true}
                        title={ee === false ? 'Available in EE mode only' : undefined}
                        checked={!!b.code_owner_approval_required}
                        onChange={() => patchBranch.mutate({ b, body: { code_owner_approval_required: !b.code_owner_approval_required } })}
                      />
                    </td>
                    <td style={{ textAlign: 'right' }}>
                      <button className="btn btn-sm btn-danger-secondary" onClick={() => unprotect.mutate(b.name)}>
                        Unprotect
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <Empty title="No protected branches" />
        )}
      </Section>
    </>
  );
}

export function ProjectAccessTokens() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Settings' }, { label: 'Access tokens' }]);
  const qc = useQueryClient();
  const base = `/projects/${project.id}/access_tokens`;
  const q = useQuery({ queryKey: ['tokens', 'project', project.id], queryFn: () => api.get<Token[]>(base, { state: 'active' }) });
  const revoke = useMutation({
    mutationFn: (t: Token) => api.del(`${base}/${t.id}`),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['tokens'] }),
  });
  return (
    <>
      <div className="page-head">
        <h1>Project access tokens</h1>
      </div>
      <p className="subtle">A project access token belongs to a bot user that is a member of this project. Use it to test integrations that act as a bot.</p>
      <TokenForm roles scopes={['api', 'read_api', 'read_repository', 'write_repository', 'create_runner', 'k8s_proxy', 'ai_features']} create={(body) => api.post<Token>(base, body)} />
      <h3 style={{ margin: '24px 0 8px' }}>Active project access tokens</h3>
      <ErrorBox error={q.error || revoke.error} />
      {q.isLoading ? <Loading /> : <TokenTable tokens={q.data ?? []} onRevoke={(t) => revoke.mutate(t)} />}
    </>
  );
}
