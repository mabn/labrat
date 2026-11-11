import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Group } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { Avatar, Empty, ErrorBox, Loading } from '../components/ui';
import { useSession } from '../session';

export function GroupList({ groups }: { groups: Group[] }) {
  return (
    <ul className="list list-bordered">
      {groups.map((g) => (
        <li key={g.id} style={{ alignItems: 'center' }}>
          <Avatar name={g.name} url={g.avatar_url} size={32} square />
          <div className="grow">
            <Link to={`/${g.full_path}`} className="title">
              {g.full_name}
            </Link>
            {g.description && <div className="meta">{g.description}</div>}
          </div>
          <span className="badge badge-muted">{g.visibility}</span>
        </li>
      ))}
    </ul>
  );
}

export function GroupsDashboard() {
  const { user } = useSession();
  const [search, setSearch] = useState('');
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'Groups' }]);
  const q = useQuery({
    queryKey: ['groups', search],
    queryFn: () => api.get<Group[]>('/groups', { search, per_page: 100, all_available: true }),
  });
  return (
    <Layout>
      <div className="page-head">
        <h1>Groups</h1>
        <div className="actions">
          {user && (
            <Link to="/groups/new" className="btn btn-confirm">
              New group
            </Link>
          )}
        </div>
      </div>
      <div className="row" style={{ marginBottom: 12, justifyContent: 'flex-end' }}>
        <input type="search" placeholder="Filter by name" style={{ width: 240 }} value={search} onChange={(e) => setSearch(e.target.value)} />
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? <Loading /> : q.data?.length ? <GroupList groups={q.data} /> : <Empty title="No groups yet" />}
    </Layout>
  );
}
