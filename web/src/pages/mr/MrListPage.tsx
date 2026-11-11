import { useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Label, MergeRequest } from '../../api/types';
import { Empty, ErrorBox, Loading, Pager } from '../../components/ui';
import { MrList } from '../../components/MrList';
import { useProject, useProjectCrumbs } from '../project/ProjectRoutes';
import { useProjectUsers } from './data';

const STATES: [string, string][] = [
  ['opened', 'Open'],
  ['merged', 'Merged'],
  ['closed', 'Closed'],
  ['all', 'All'],
];

export function MrListPage() {
  const { project } = useProject();
  useProjectCrumbs([{ label: 'Merge requests' }]);
  const [params, setParams] = useSearchParams();
  const state = params.get('state') ?? 'opened';
  const page = Number(params.get('page') ?? 1);
  const search = params.get('search') ?? '';
  const author = params.get('author_username') ?? '';
  const reviewer = params.get('reviewer_username') ?? '';
  const label = params.get('label_name') ?? '';
  const sort = params.get('sort') ?? 'created_at_desc';
  const [searchText, setSearchText] = useState(search);
  const set = (k: string, v: string) => {
    const next = new URLSearchParams(params);
    if (v) next.set(k, v);
    else next.delete(k);
    if (k !== 'page') next.delete('page');
    setParams(next);
  };
  const users = useProjectUsers(project);
  const labels = useQuery({ queryKey: ['labels', `/projects/${project.id}`], queryFn: () => api.all<Label>(`/projects/${project.id}/labels`, { include_ancestor_groups: true }) });
  const [orderBy, dir] = sort.split(/_(?=asc$|desc$)/);
  const filters = { search, author_username: author, reviewer_username: reviewer, labels: label, order_by: orderBy, sort: dir };
  const q = useQuery({
    queryKey: ['mrs', project.id, state, page, filters],
    queryFn: () => api.page<MergeRequest>(`/projects/${project.id}/merge_requests`, { state, page, per_page: 20, ...filters , with_labels_details: true }),
  });
  const counts = useQuery({
    queryKey: ['mrs', project.id, 'counts', filters],
    queryFn: async () => {
      const out: Record<string, number | null> = {};
      await Promise.all(STATES.map(async ([s]) => (out[s] = (await api.page(`/projects/${project.id}/merge_requests`, { state: s, per_page: 1, ...filters })).total)));
      return out;
    },
  });
  return (
    <>
      <div className="page-head">
        <h1>Merge requests</h1>
        <div className="actions">
          <Link className="btn btn-confirm" to={`/${project.path_with_namespace}/-/merge_requests/new`}>
            New merge request
          </Link>
        </div>
      </div>
      <div className="tabs">
        {STATES.map(([s, l]) => (
          <button key={s} className={`tab${state === s ? ' active' : ''}`} onClick={() => set('state', s === 'opened' ? '' : s)}>
            {l} {counts.data?.[s] !== undefined && counts.data?.[s] !== null && <span className="count">{counts.data[s]}</span>}
          </button>
        ))}
      </div>
      <form
        className="filter-bar"
        onSubmit={(e) => {
          e.preventDefault();
          set('search', searchText);
        }}
      >
        <input type="search" className="grow" style={{ minWidth: 200, flex: 1 }} placeholder="Search or filter results…" value={searchText} onChange={(e) => setSearchText(e.target.value)} />
        <select style={{ width: 170 }} value={author} onChange={(e) => set('author_username', e.target.value)}>
          <option value="">Any author</option>
          {users.data?.map((u) => (
            <option key={u.id} value={u.username}>
              Author: {u.name}
            </option>
          ))}
        </select>
        <select style={{ width: 170 }} value={reviewer} onChange={(e) => set('reviewer_username', e.target.value)}>
          <option value="">Any reviewer</option>
          {users.data?.map((u) => (
            <option key={u.id} value={u.username}>
              Reviewer: {u.name}
            </option>
          ))}
        </select>
        <select style={{ width: 150 }} value={label} onChange={(e) => set('label_name', e.target.value)}>
          <option value="">Any label</option>
          {labels.data?.map((l) => (
            <option key={l.id} value={l.name}>
              {l.name}
            </option>
          ))}
        </select>
        <select style={{ width: 170 }} value={sort} onChange={(e) => set('sort', e.target.value === 'created_at_desc' ? '' : e.target.value)}>
          <option value="created_at_desc">Created date (newest)</option>
          <option value="created_at_asc">Created date (oldest)</option>
          <option value="updated_at_desc">Updated date</option>
          <option value="title_asc">Title</option>
          <option value="merged_at_desc">Merged date</option>
        </select>
      </form>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <div className="list-bordered">
          <Loading />
        </div>
      ) : q.data?.items.length ? (
        <>
          <MrList mrs={q.data.items} />
          <Pager page={q.data.page} totalPages={q.data.totalPages} nextPage={q.data.nextPage} onPage={(p) => set('page', String(p))} />
        </>
      ) : (
        <div className="list-bordered">
          <Empty title={state === 'opened' && !search ? 'Make collaboration easier with merge requests' : 'No merge requests found'}>
            {state === 'opened' && (
              <Link className="btn btn-confirm" to={`/${project.path_with_namespace}/-/merge_requests/new`}>
                New merge request
              </Link>
            )}
          </Empty>
        </div>
      )}
    </>
  );
}
