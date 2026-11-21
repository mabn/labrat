import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Project } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { Avatar, Empty, ErrorBox, Loading, Pager, TimeAgo } from '../components/ui';
import { Icon } from '../components/Icon';
import { useSession } from '../session';
import { localPath } from '../lib/paths';

export function ProjectList({ projects }: { projects: Project[] }) {
  return (
    <ul className="list list-bordered">
      {projects.map((p) => (
        <li key={p.id} style={{ alignItems: 'center' }}>
          <Avatar name={p.name} size={32} square />
          <div className="grow">
            <Link to={localPath(p.web_url)} className="title">
              <span className="subtle" style={{ fontWeight: 400 }}>{p.namespace.full_path} / </span>
              {p.name}
            </Link>
            {p.description && <div className="meta truncate">{p.description}</div>}
          </div>
          <span className="badge badge-muted" title="Visibility">
            <Icon name={p.visibility === 'private' ? 'lock' : 'eye'} size={12} /> {p.visibility}
          </span>
          <span className="small subtle" style={{ width: 140, textAlign: 'right' }}>
            Updated <TimeAgo at={p.last_activity_at} />
          </span>
        </li>
      ))}
    </ul>
  );
}

export function ProjectsDashboard({ scope }: { scope: 'mine' | 'all' }) {
  const { user } = useSession();
  const [search, setSearch] = useState('');
  const [page, setPage] = useState(1);
  const [tab, setTab] = useState<'mine' | 'all'>(scope);
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'Projects' }]);
  const q = useQuery({
    queryKey: ['projects', tab, search, page],
    queryFn: () =>
      api.page<Project>('/projects', {
        membership: tab === 'mine' ? true : undefined,
        search,
        order_by: 'last_activity_at',
        per_page: 20,
        page,
      }),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Projects</h1>
        <div className="actions">
          {user && (
            <Link to="/projects/new" className="btn btn-confirm">
              New project
            </Link>
          )}
        </div>
      </div>
      <div className="row" style={{ marginBottom: 12 }}>
        <div className="tabs grow" style={{ marginBottom: 0 }}>
          {user && (
            <button className={`tab${tab === 'mine' ? ' active' : ''}`} onClick={() => { setTab('mine'); setPage(1); }}>
              Yours
            </button>
          )}
          <button className={`tab${tab === 'all' ? ' active' : ''}`} onClick={() => { setTab('all'); setPage(1); }}>
            All
          </button>
        </div>
        <input type="search" placeholder="Filter by name" style={{ width: 240 }} value={search} onChange={(e) => { setSearch(e.target.value); setPage(1); }} />
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data && q.data.items.length ? (
        <>
          <ProjectList projects={q.data.items} />
          <Pager page={q.data.page} totalPages={q.data.totalPages} nextPage={q.data.nextPage} onPage={setPage} />
        </>
      ) : (
        <Empty title={search ? 'No projects match your search' : 'No projects yet'}>
          {user && !search && (
            <Link className="btn btn-confirm" to="/projects/new">
              Create a project
            </Link>
          )}
        </Empty>
      )}
    </Layout>
  );
}
