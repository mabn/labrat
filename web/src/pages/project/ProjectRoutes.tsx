import { createContext, useContext } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Project } from '../../api/types';
import { Layout, useCrumbs, type Crumb, type SidebarContext } from '../../components/Layout';
import { Empty } from '../../components/ui';
import { useSession } from '../../session';
import { Overview } from './Overview';
import { Tree, Blob, EditFile } from './Files';
import { Commits, CommitPage } from './Commits';
import { Branches, Tags } from './Branches';
import { GeneralSettings, MergeRequestSettings, RepositorySettings, ProjectAccessTokens } from './Settings';
import { Milestones } from './Milestones';
import { Members } from '../../components/Members';
import { Labels } from '../../components/Labels';
import { Webhooks } from '../../components/Webhooks';
import { MrListPage } from '../mr/MrListPage';
import { NewMr } from '../mr/NewMr';
import { MrPage } from '../mr/MrPage';
import { MergeTrains } from './MergeTrains';
import { TestSetup } from './TestSetup';

interface ProjectCtx {
  project: Project;
  /** The user's access level (0 when not a member; 60 stands for an administrator). */
  access: number;
  canRead: boolean;
  canWrite: boolean;
  canMaintain: boolean;
}

const Ctx = createContext<ProjectCtx | null>(null);
export const useProject = () => useContext(Ctx)!;

/** Sets breadcrumbs under the project's own. */
export function useProjectCrumbs(extra: Crumb[]) {
  const { project } = useProject();
  const ns = project.namespace.full_path.split('/').map((seg, i, all) => ({ label: seg, to: `/${all.slice(0, i + 1).join('/')}` }));
  useCrumbs([...ns, { label: project.name, to: `/${project.path_with_namespace}` }, ...extra]);
}

export function ProjectRoutes({ project: initial, rest }: { project: Project; rest: string }) {
  const { user } = useSession();
  const q = useQuery({
    queryKey: ['project', initial.id],
    queryFn: () => api.get<Project>(`/projects/${initial.id}`, { license: false }),
    initialData: initial,
    staleTime: 10_000,
  });
  const project = q.data ?? initial;
  const perms = project.permissions;
  const memberLevel = Math.max(perms?.project_access?.access_level ?? 0, perms?.group_access?.access_level ?? 0);
  const access = user?.is_admin ? 60 : memberLevel;
  const ctx: ProjectCtx = {
    project,
    access,
    canRead: true,
    canWrite: access >= 30,
    canMaintain: access >= 40,
  };
  const counts = useQuery({
    queryKey: ['mr-count', project.id],
    queryFn: async () => (await api.page(`/projects/${project.id}/merge_requests`, { state: 'opened', per_page: 1 })).total,
    staleTime: 30_000,
  });
  const p = `/${project.path_with_namespace}`;
  const sidebar: SidebarContext = {
    title: project.name,
    to: p,
    avatar: { name: project.name },
    nav: [
      { label: 'Project overview', to: p, icon: 'project', exact: true },
      {
        label: 'Code',
        to: `${p}/-/tree`,
        icon: 'code',
        sub: [
          { label: 'Repository', to: `${p}/-/tree` },
          { label: 'Branches', to: `${p}/-/branches` },
          { label: 'Commits', to: `${p}/-/commits` },
          { label: 'Tags', to: `${p}/-/tags` },
        ],
      },
      {
        label: 'Merge requests',
        to: `${p}/-/merge_requests`,
        icon: 'mr',
        count: counts.data ?? undefined,
        sub: [
          { label: 'Merge requests', to: `${p}/-/merge_requests` },
          { label: 'Merge trains', to: `${p}/-/merge_trains` },
        ],
      },
      {
        label: 'Manage',
        to: `${p}/-/project_members`,
        icon: 'users',
        sub: [
          { label: 'Members', to: `${p}/-/project_members` },
          { label: 'Labels', to: `${p}/-/labels` },
          { label: 'Milestones', to: `${p}/-/milestones` },
        ],
      },
      ...(ctx.canMaintain
        ? [
            {
              label: 'Settings',
              to: `${p}/-/settings`,
              icon: 'settings',
              sub: [
                { label: 'General', to: `${p}/-/settings/general` },
                { label: 'Repository', to: `${p}/-/settings/repository` },
                { label: 'Merge requests', to: `${p}/-/settings/merge_requests` },
                { label: 'Webhooks', to: `${p}/-/settings/webhooks` },
                { label: 'Access tokens', to: `${p}/-/settings/access_tokens` },
                { label: 'labrat test setup', to: `${p}/-/settings/labrat` },
              ],
            },
          ]
        : []),
    ],
  };
  const parts = rest.split('/').filter(Boolean);
  const wide = parts[0] === 'merge_requests' && (parts[2] === 'diffs' || parts[1] === 'new');
  return (
    <Ctx.Provider value={ctx}>
      <Layout context={sidebar} wide={wide}>
        <Page parts={parts} />
      </Layout>
    </Ctx.Provider>
  );
}

function Simple({ title, children }: { title: string; children: React.ReactNode }) {
  useProjectCrumbs([{ label: title }]);
  return <>{children}</>;
}

function Page({ parts }: { parts: string[] }) {
  const { project, canMaintain, canWrite } = useProject();
  const base = `/projects/${project.id}`;
  const [head, ...tail] = parts;
  switch (head) {
    case undefined:
      return <Overview />;
    case 'tree':
      return <Tree parts={tail} />;
    case 'blob':
      return <Blob parts={tail} />;
    case 'edit':
      return <EditFile parts={tail} mode="edit" />;
    case 'new':
      return <EditFile parts={tail} mode="new" />;
    case 'commits':
      return <Commits parts={tail} />;
    case 'commit':
      return <CommitPage sha={tail[0]} />;
    case 'branches':
      return <Branches />;
    case 'tags':
      return <Tags />;
    case 'merge_requests':
      if (!tail.length) return <MrListPage />;
      if (tail[0] === 'new') return <NewMr />;
      return <MrPage key={tail[0]} iid={Number(tail[0])} tab={tail[1] ?? ''} />;
    case 'merge_trains':
      return <MergeTrains />;
    case 'project_members':
      return (
        <Simple title="Members">
          <div className="page-head">
            <h1>Project members</h1>
          </div>
          <Members base={base} canManage={canMaintain} />
        </Simple>
      );
    case 'labels':
      return (
        <Simple title="Labels">
          <Labels base={base} canManage={canWrite} />
        </Simple>
      );
    case 'milestones':
      return <Milestones />;
    case 'hooks':
      return <Simple title="Webhooks"><Webhooks base={base} /></Simple>;
    case 'edit_project':
    case 'settings':
      switch (tail[0]) {
        case 'repository':
          return <RepositorySettings />;
        case 'merge_requests':
          return <MergeRequestSettings />;
        case 'webhooks':
        case 'integrations':
          return <Simple title="Webhooks"><Webhooks base={base} /></Simple>;
        case 'access_tokens':
          return <ProjectAccessTokens />;
        case 'labrat':
          return <TestSetup />;
        default:
          return <GeneralSettings />;
      }
    default:
      return (
        <Simple title="Not found">
          <Empty title="404: Page not found" />
        </Simple>
      );
  }
}
