import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { Event, Project, User } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { Empty, ErrorBox, Loading, TimeAgo, UserAvatar } from '../components/ui';
import { ProjectList } from './ProjectsDashboard';

export function UserPage({ user }: { user: User }) {
  useCrumbs([{ label: user.name }]);
  const projects = useQuery({ queryKey: ['user-projects', user.id], queryFn: () => api.get<Project[]>(`/users/${user.id}/projects`, { per_page: 50 }) });
  const events = useQuery({ queryKey: ['user-events', user.id], queryFn: () => api.get<Event[]>(`/users/${user.id}/events`, { per_page: 30 }) });
  return (
    <Layout>
      <div className="page-head">
        <UserAvatar user={user} size={64} />
        <div>
          <h1>{user.name}</h1>
          <div className="subtle">
            @{user.username} {user.bot && <span className="badge">Bot</span>} · Member since <TimeAgo at={user.created_at} />
          </div>
        </div>
      </div>
      {user.bio && <p>{user.bio}</p>}
      <div className="row" style={{ alignItems: 'flex-start', gap: 24 }}>
        <div className="grow">
          <h3 style={{ marginBottom: 8 }}>Personal projects</h3>
          <ErrorBox error={projects.error} />
          {projects.isLoading ? <Loading /> : projects.data?.length ? <ProjectList projects={projects.data} /> : <Empty title="No personal projects" />}
        </div>
        <div style={{ width: 380 }}>
          <h3 style={{ marginBottom: 8 }}>Activity</h3>
          {events.isLoading ? (
            <Loading />
          ) : events.data?.length ? (
            <ul className="list list-bordered">
              {events.data.map((e) => (
                <li key={e.id} className="small">
                  <div className="grow">
                    {e.action_name} {e.target_type && <span>{e.target_type.replace(/([A-Z])/g, ' $1').trim().toLowerCase()}</span>}{' '}
                    {e.target_title && <strong>{e.target_title}</strong>}
                    {e.push_data && (
                      <span>
                        {' '}
                        {e.push_data.ref_type} <code>{e.push_data.ref}</code>
                      </span>
                    )}
                    <div className="subtle">
                      <TimeAgo at={e.created_at} />
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <Empty title="No activity" />
          )}
        </div>
      </div>
    </Layout>
  );
}
