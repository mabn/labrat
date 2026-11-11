import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from '../api/client';
import type { MergeRequest } from '../api/types';
import { Layout, useCrumbs } from '../components/Layout';
import { Empty, ErrorBox, Loading, Pager } from '../components/ui';
import { MrList } from '../components/MrList';
import { useSession } from '../session';

type Tab = 'assigned' | 'review' | 'created';

export function MergeRequestsDashboard() {
  const { user } = useSession();
  const [tab, setTab] = useState<Tab>('review');
  const [state, setState] = useState('opened');
  const [page, setPage] = useState(1);
  useCrumbs([{ label: 'Your work', to: '/' }, { label: 'Merge requests' }]);
  const query =
    tab === 'assigned'
      ? { scope: 'assigned_to_me' }
      : tab === 'review'
        ? { scope: 'all', reviewer_username: user!.username }
        : { scope: 'created_by_me' };
  const q = useQuery({
    queryKey: ['dash-mrs', tab, state, page],
    queryFn: () => api.page<MergeRequest>('/merge_requests', { ...query, state, per_page: 20, page , with_labels_details: true }),
  });
  const tabs: [Tab, string][] = [
    ['review', 'Review requested'],
    ['assigned', 'Assigned to you'],
    ['created', 'Created by you'],
  ];
  return (
    <Layout>
      <div className="page-head">
        <h1>Merge requests</h1>
      </div>
      <div className="row" style={{ marginBottom: 12 }}>
        <div className="tabs grow" style={{ marginBottom: 0 }}>
          {tabs.map(([t, label]) => (
            <button key={t} className={`tab${tab === t ? ' active' : ''}`} onClick={() => { setTab(t); setPage(1); }}>
              {label}
            </button>
          ))}
        </div>
        <select style={{ width: 140 }} value={state} onChange={(e) => { setState(e.target.value); setPage(1); }}>
          <option value="opened">Open</option>
          <option value="merged">Merged</option>
          <option value="closed">Closed</option>
          <option value="all">All</option>
        </select>
      </div>
      <ErrorBox error={q.error} />
      {q.isLoading ? (
        <Loading />
      ) : q.data?.items.length ? (
        <>
          <MrList mrs={q.data.items} showProject />
          <Pager page={q.data.page} totalPages={q.data.totalPages} nextPage={q.data.nextPage} onPage={setPage} />
        </>
      ) : (
        <Empty title="No merge requests here" />
      )}
    </Layout>
  );
}
