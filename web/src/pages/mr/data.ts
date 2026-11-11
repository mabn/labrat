import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Approvals, Award, Discussion, DraftNote, MergeRequest, MrVersion, Project, UserBasic } from '../../api/types';

export const mrBase = (projectId: number, iid: number) => `/projects/${projectId}/merge_requests/${iid}`;

const busy = (mr?: MergeRequest) =>
  !!mr && mr.state === 'opened' && (['unchecked', 'checking', 'preparing', 'approvals_syncing'].includes(mr.detailed_merge_status) || mr.merge_status === 'checking' || mr.merge_status === 'unchecked' || !!mr.rebase_in_progress);

export function useMr(project: Project, iid: number) {
  return useQuery({
    queryKey: ['mr', project.id, iid],
    queryFn: () => api.get<MergeRequest>(mrBase(project.id, iid), { include_diverged_commits_count: true, include_rebase_in_progress: true }),
    // Mergeability and rebases are computed in the background; poll until they settle.
    refetchInterval: (q) => (busy(q.state.data) ? 1500 : false),
  });
}

export function useApprovals(project: Project, iid: number) {
  return useQuery({ queryKey: ['mr', project.id, iid, 'approvals'], queryFn: () => api.get<Approvals>(`${mrBase(project.id, iid)}/approvals`) });
}

export function useDiscussions(project: Project, iid: number) {
  return useQuery({ queryKey: ['mr', project.id, iid, 'discussions'], queryFn: () => api.all<Discussion>(`${mrBase(project.id, iid)}/discussions`) });
}

export function useDrafts(project: Project, iid: number, enabled: boolean) {
  return useQuery({ queryKey: ['mr', project.id, iid, 'drafts'], queryFn: () => api.all<DraftNote>(`${mrBase(project.id, iid)}/draft_notes`), enabled });
}

export function useMrAwards(project: Project, iid: number) {
  return useQuery({ queryKey: ['mr', project.id, iid, 'awards'], queryFn: () => api.all<Award>(`${mrBase(project.id, iid)}/award_emoji`) });
}

export function useVersions(project: Project, iid: number) {
  return useQuery({ queryKey: ['mr', project.id, iid, 'versions'], queryFn: () => api.get<MrVersion[]>(`${mrBase(project.id, iid)}/versions`) });
}

export function useProjectUsers(project: Project) {
  return useQuery({ queryKey: ['project-users', project.id], queryFn: () => api.all<UserBasic>(`/projects/${project.id}/users`), staleTime: 60_000 });
}

/** Awards on every note of the MR, in one GraphQL query (REST needs one call per note). */
export function useNoteAwards(project: Project, iid: number) {
  return useQuery({
    queryKey: ['mr', project.id, iid, 'note-awards'],
    queryFn: async () => {
      const out = new Map<number, { name: string; username: string }[]>();
      let after: string | null = null;
      for (let i = 0; i < 20; i++) {
        type R = { project: { mergeRequest: { notes: { pageInfo: { hasNextPage: boolean; endCursor: string | null }; nodes: { id: string; awardEmoji: { nodes: { name: string; user: { username: string } }[] } }[] } } | null } | null };
        const data: R = await api.graphql<R>(
          `query($path: ID!, $iid: String!, $after: String) {
            project(fullPath: $path) { mergeRequest(iid: $iid) { notes(first: 100, after: $after) {
              pageInfo { hasNextPage endCursor }
              nodes { id awardEmoji { nodes { name user { username } } } }
            } } }
          }`,
          { path: project.path_with_namespace, iid: String(iid), after },
        );
        const notes = data.project?.mergeRequest?.notes;
        if (!notes) break;
        for (const n of notes.nodes) {
          const id = Number(n.id.split('/').pop());
          out.set(id, n.awardEmoji.nodes.map((a) => ({ name: a.name, username: a.user.username })));
        }
        if (!notes.pageInfo.hasNextPage) break;
        after = notes.pageInfo.endCursor;
      }
      return out;
    },
    retry: false,
  });
}

/** Refreshes everything about one MR after a change. */
export function useMrRefresh(project: Project, iid: number) {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: ['mr', project.id, iid] });
    qc.invalidateQueries({ queryKey: ['mrs', project.id] });
    qc.invalidateQueries({ queryKey: ['mr-count', project.id] });
    qc.invalidateQueries({ queryKey: ['user_counts'] });
  };
}
