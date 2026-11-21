import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Group, MergeRequest, Project } from '../api/types';
import { Layout, useCrumbs, type SidebarContext } from '../components/Layout';
import { Avatar, Empty, ErrorBox, Loading, Pager, useToast } from '../components/ui';
import { Members } from '../components/Members';
import { Labels } from '../components/Labels';
import { MrList } from '../components/MrList';
import { ProjectList } from './ProjectsDashboard';
import { GroupList } from './GroupsDashboard';
import { useSession } from '../session';

export function GroupPage({ group, sub }: { group: Group; sub: string }) {
  const base = `/groups/${group.id}`;
  const { user } = useSession();
  const access = useQuery({
    queryKey: ['group-access', group.id, user?.id],
    queryFn: async () => {
      if (!user) return 0;
      try {
        const m = await api.get<{ access_level: number }>(`${base}/members/all/${user.id}`);
        return m.access_level;
      } catch {
        return 0;
      }
    },
  });
  const level = user?.is_admin ? 50 : (access.data ?? 0);
  const ctx: SidebarContext = {
    title: group.name,
    to: `/${group.full_path}`,
    avatar: { name: group.name, url: group.avatar_url },
    nav: [
      { label: 'Group overview', to: `/${group.full_path}`, icon: 'group', exact: true },
      { label: 'Merge requests', to: `/groups/${group.full_path}/-/merge_requests`, icon: 'mr' },
      { label: 'Members', to: `/groups/${group.full_path}/-/group_members`, icon: 'users' },
      { label: 'Labels', to: `/groups/${group.full_path}/-/labels`, icon: 'label' },
      ...(level >= 50 ? [{ label: 'Settings', to: `/groups/${group.full_path}/-/edit`, icon: 'settings' }] : []),
    ],
  };
  const crumbs = group.full_path.split('/').map((seg, i, all) => ({ label: seg, to: `/${all.slice(0, i + 1).join('/')}` }));
  let body;
  let title = '';
  switch (sub.replace(/\/$/, '')) {
    case 'merge_requests':
      title = 'Merge requests';
      body = <GroupMrs base={base} />;
      break;
    case 'group_members':
      title = 'Members';
      body = (
        <>
          <div className="page-head">
            <h1>Group members</h1>
          </div>
          <Members base={base} canManage={level >= 50} />
        </>
      );
      break;
    case 'labels':
      title = 'Labels';
      body = <Labels base={base} canManage={level >= 30} />;
      break;
    case 'edit':
      title = 'Settings';
      body = <GroupSettings group={group} />;
      break;
    default:
      body = <GroupOverview group={group} canCreate={level >= 30} />;
  }
  return (
    <Layout context={ctx}>
      <Crumbs crumbs={title ? [...crumbs, { label: title }] : crumbs} />
      {body}
    </Layout>
  );
}

function Crumbs({ crumbs }: { crumbs: { label: string; to?: string }[] }) {
  useCrumbs(crumbs);
  return null;
}

function GroupOverview({ group, canCreate }: { group: Group; canCreate: boolean }) {
  const base = `/groups/${group.id}`;
  const projects = useQuery({ queryKey: ['group-projects', group.id], queryFn: () => api.get<Project[]>(`${base}/projects`, { per_page: 100, include_subgroups: false }) });
  const subgroups = useQuery({ queryKey: ['subgroups', group.id], queryFn: () => api.get<Group[]>(`${base}/subgroups`, { per_page: 100 }) });
  return (
    <>
      <div className="page-head">
        <Avatar name={group.name} url={group.avatar_url} size={48} square />
        <div>
          <h1>{group.name}</h1>
          <div className="subtle small">Group ID: {group.id}</div>
        </div>
        <div className="actions">
          {canCreate && (
            <>
              <Link className="btn" to={`/groups/new?parent_id=${group.id}`}>
                New subgroup
              </Link>
              <Link className="btn btn-confirm" to={`/projects/new?namespace_id=${group.id}`}>
                New project
              </Link>
            </>
          )}
        </div>
      </div>
      {group.description && <p>{group.description}</p>}
      <ErrorBox error={projects.error || subgroups.error} />
      {subgroups.data && subgroups.data.length > 0 && (
        <>
          <h3 style={{ margin: '16px 0 8px' }}>Subgroups</h3>
          <GroupList groups={subgroups.data} />
        </>
      )}
      <h3 style={{ margin: '16px 0 8px' }}>Projects</h3>
      {projects.isLoading ? <Loading /> : projects.data?.length ? <ProjectList projects={projects.data} /> : <Empty title="There are no projects in this group" />}
    </>
  );
}

function GroupMrs({ base }: { base: string }) {
  const [state, setState] = useState('opened');
  const [page, setPage] = useState(1);
  const q = useQuery({ queryKey: ['group-mrs', base, state, page], queryFn: () => api.page<MergeRequest>(`${base}/merge_requests`, { state, page, per_page: 20 , with_labels_details: true }) });
  return (
    <>
      <div className="page-head">
        <h1>Merge requests</h1>
      </div>
      <div className="tabs">
        {[
          ['opened', 'Open'],
          ['merged', 'Merged'],
          ['closed', 'Closed'],
          ['all', 'All'],
        ].map(([s, l]) => (
          <button key={s} className={`tab${state === s ? ' active' : ''}`} onClick={() => { setState(s); setPage(1); }}>
            {l}
          </button>
        ))}
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? <Loading /> : q.data?.items.length ? (
        <>
          <MrList mrs={q.data.items} showProject />
          <Pager page={q.data.page} totalPages={q.data.totalPages} nextPage={q.data.nextPage} onPage={setPage} />
        </>
      ) : (
        <Empty title="No merge requests" />
      )}
    </>
  );
}

function GroupSettings({ group }: { group: Group }) {
  const qc = useQueryClient();
  const toast = useToast();
  const nav = useNavigate();
  const [name, setName] = useState(group.name);
  const [description, setDescription] = useState(group.description ?? '');
  const [visibility, setVisibility] = useState(group.visibility);
  const save = useMutation({
    mutationFn: () => api.put<Group>(`/groups/${group.id}`, { name, description, visibility }),
    onSuccess: () => {
      toast('Group settings saved');
      qc.invalidateQueries({ queryKey: ['resolve'] });
      qc.invalidateQueries({ queryKey: ['group'] });
    },
  });
  const remove = useMutation({
    mutationFn: () => api.del(`/groups/${group.id}`),
    onSuccess: () => {
      toast('Group scheduled for deletion');
      nav('/dashboard/groups');
    },
  });
  return (
    <>
      <div className="page-head">
        <h1>General settings</h1>
      </div>
      <form
        className="form-narrow"
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate();
        }}
      >
        <ErrorBox error={save.error} />
        <label className="field">
          <span className="label">Group name</span>
          <input type="text" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="field">
          <span className="label">Group description</span>
          <textarea rows={3} value={description} onChange={(e) => setDescription(e.target.value)} />
        </label>
        <label className="field">
          <span className="label">Visibility level</span>
          <select value={visibility} onChange={(e) => setVisibility(e.target.value)}>
            <option value="private">Private</option>
            <option value="internal">Internal</option>
            <option value="public">Public</option>
          </select>
        </label>
        <button className="btn btn-confirm" disabled={save.isPending}>
          Save changes
        </button>
      </form>
      <hr />
      <div className="card" style={{ borderColor: 'var(--red-100)' }}>
        <div className="card-head" style={{ color: 'var(--red-600)' }}>
          Delete group
        </div>
        <div className="card-body">
          <ErrorBox error={remove.error} />
          <p>This deletes the group, its subgroups and projects.</p>
          <button className="btn btn-danger" onClick={() => confirm(`Delete group ${group.full_path}?`) && remove.mutate()}>
            Delete group
          </button>
        </div>
      </div>
    </>
  );
}
