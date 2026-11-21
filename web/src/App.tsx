import { Navigate, Route, Routes, useLocation, useParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api, ApiError, enc } from './api/client';
import type { Group, Project, User } from './api/types';
import { useSession } from './session';
import { Layout } from './components/Layout';
import { ErrorBox, Loading, Empty } from './components/ui';
import { SignIn } from './pages/SignIn';
import { ProjectsDashboard } from './pages/ProjectsDashboard';
import { GroupsDashboard } from './pages/GroupsDashboard';
import { MergeRequestsDashboard } from './pages/MergeRequestsDashboard';
import { Todos } from './pages/Todos';
import { NewProject } from './pages/NewProject';
import { NewGroup } from './pages/NewGroup';
import { GroupPage } from './pages/GroupPage';
import { UserPage } from './pages/UserPage';
import { AccessTokens } from './pages/AccessTokens';
import { RequestLog } from './pages/RequestLog';
import { AdminUsers, AdminNewUser } from './pages/admin/Users';
import { SystemHooks } from './pages/admin/SystemHooks';
import { AdminSettings } from './pages/admin/Settings';
import { ProjectRoutes } from './pages/project/ProjectRoutes';

export function App() {
  const { user, loading } = useSession();
  if (loading) return <Loading />;
  return (
    <Routes>
      <Route path="/users/sign_in" element={<SignIn />} />
      <Route path="/" element={user ? <Navigate to="/dashboard/projects" replace /> : <Navigate to="/explore/projects" replace />} />
      <Route path="/dashboard/projects" element={<RequireUser><ProjectsDashboard scope="mine" /></RequireUser>} />
      <Route path="/explore/projects" element={<ProjectsDashboard scope="all" />} />
      <Route path="/dashboard/groups" element={<RequireUser><GroupsDashboard /></RequireUser>} />
      <Route path="/explore/groups" element={<GroupsDashboard />} />
      <Route path="/dashboard/merge_requests" element={<RequireUser><MergeRequestsDashboard /></RequireUser>} />
      <Route path="/dashboard/todos" element={<RequireUser><Todos /></RequireUser>} />
      <Route path="/projects/new" element={<RequireUser><NewProject /></RequireUser>} />
      <Route path="/groups/new" element={<RequireUser><NewGroup /></RequireUser>} />
      <Route path="/admin" element={<Navigate to="/admin/users" replace />} />
      <Route path="/admin/users" element={<RequireUser admin><AdminUsers /></RequireUser>} />
      <Route path="/admin/users/new" element={<RequireUser admin><AdminNewUser /></RequireUser>} />
      <Route path="/admin/hooks" element={<RequireUser admin><SystemHooks /></RequireUser>} />
      <Route path="/admin/application_settings/network" element={<RequireUser admin><AdminSettings /></RequireUser>} />
      <Route path="/-/user_settings/personal_access_tokens" element={<RequireUser><AccessTokens /></RequireUser>} />
      <Route path="/-/profile/personal_access_tokens" element={<Navigate to="/-/user_settings/personal_access_tokens" replace />} />
      <Route path="/-/labrat/requests" element={<RequestLog />} />
      <Route path="/groups/*" element={<GroupsPrefixed />} />
      <Route path="/*" element={<Resolve />} />
    </Routes>
  );
}

function RequireUser({ children, admin }: { children: React.ReactNode; admin?: boolean }) {
  const { user } = useSession();
  const loc = useLocation();
  if (!user) return <Navigate to={`/users/sign_in?redirect=${encodeURIComponent(loc.pathname + loc.search)}`} replace />;
  if (admin && !user.is_admin) return <NotFound />;
  return <>{children}</>;
}

export function NotFound() {
  return (
    <Layout>
      <Empty title="404: Page not found">
        <p>Make sure the address is correct and the page hasn't moved.</p>
      </Empty>
    </Layout>
  );
}

/** `/groups/<path>[/-/<page>]`, GitLab's URL for group pages. */
function GroupsPrefixed() {
  const params = useParams();
  const rest = params['*'] ?? '';
  const [path, sub] = rest.split('/-/');
  return <GroupResolver path={path.replace(/\/$/, '')} sub={sub ?? ''} />;
}

function GroupResolver({ path, sub }: { path: string; sub: string }) {
  const q = useQuery({ queryKey: ['group', path], queryFn: () => api.get<Group>(`/groups/${enc(path)}`, { with_projects: false }) });
  if (q.isLoading) return <Layout><Loading /></Layout>;
  if (q.error) return q.error instanceof ApiError && q.error.status === 404 ? <NotFound /> : <Layout><ErrorBox error={q.error} /></Layout>;
  return <GroupPage group={q.data!} sub={sub} />;
}

type Resolved = { kind: 'project'; project: Project } | { kind: 'group'; group: Group } | { kind: 'user'; user: User } | { kind: 'none' };

/**
 * Any other path is `<namespace path>` or `<project path>/-/<page>`, as on GitLab. Which one
 * a path names is found by asking the API.
 */
function Resolve() {
  const params = useParams();
  const full = (params['*'] ?? '').replace(/\/$/, '');
  const sep = full.indexOf('/-/');
  const path = sep >= 0 ? full.slice(0, sep) : full.replace(/\/-$/, '');
  const rest = sep >= 0 ? full.slice(sep + 3) : '';
  const q = useQuery({
    queryKey: ['resolve', path],
    queryFn: async (): Promise<Resolved> => {
      const notFound = (e: unknown) => e instanceof ApiError && (e.status === 404 || e.status === 401 || e.status === 403);
      try {
        return { kind: 'project', project: await api.get<Project>(`/projects/${enc(path)}`, { license: false }) };
      } catch (e) {
        if (!notFound(e)) throw e;
      }
      if (rest) return { kind: 'none' };
      try {
        return { kind: 'group', group: await api.get<Group>(`/groups/${enc(path)}`, { with_projects: false }) };
      } catch (e) {
        if (!notFound(e)) throw e;
      }
      if (!path.includes('/')) {
        const users = await api.get<User[]>('/users', { username: path });
        if (users.length) return { kind: 'user', user: users[0] };
      }
      return { kind: 'none' };
    },
    enabled: !!path,
  });
  if (!path) return <NotFound />;
  if (q.isLoading) return <Layout><Loading /></Layout>;
  if (q.error) return <Layout><ErrorBox error={q.error} /></Layout>;
  const r = q.data!;
  if (r.kind === 'project') return <ProjectRoutes project={r.project} rest={rest} />;
  if (r.kind === 'group') return <GroupPage group={r.group} sub="" />;
  if (r.kind === 'user') return <UserPage user={r.user} />;
  return <NotFound />;
}
