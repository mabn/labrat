// EE merge trains: the project's trains, one per target branch. Disabled in CE mode.

import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { MergeTrainCar } from '../../api/types';
import { CiStatus, ErrorBox, Loading, TimeAgo, UserAvatar } from '../../components/ui';
import { EeBadge, EeOnly, useEdition } from '../../edition';
import { useProject, useProjectCrumbs } from './ProjectRoutes';

export function MergeTrains() {
  const { project } = useProject();
  const { ee } = useEdition();
  useProjectCrumbs([{ label: 'Merge trains' }]);
  const [scope, setScope] = useState<'active' | 'complete'>('active');
  const q = useQuery({
    queryKey: ['merge-trains', project.id, scope],
    queryFn: () => api.all<MergeTrainCar>(`/projects/${project.id}/merge_trains`, { scope, sort: scope === 'active' ? 'asc' : 'desc' }, 5),
    enabled: ee === true,
    refetchInterval: scope === 'active' ? 5000 : false,
  });
  const byBranch = new Map<string, MergeTrainCar[]>();
  for (const car of q.data ?? []) byBranch.set(car.target_branch, [...(byBranch.get(car.target_branch) ?? []), car]);
  const p = `/${project.path_with_namespace}`;
  return (
    <>
      <div className="page-head">
        <h1>
          Merge trains
          <EeBadge />
        </h1>
      </div>
      <EeOnly>
        {!project.merge_trains_enabled && ee && (
          <div className="alert alert-info">
            Merge trains are off for this project. Turn them on in <Link to={`${p}/-/settings/merge_requests`}>Settings › Merge requests</Link>.
          </div>
        )}
        <div className="tabs">
          {(['active', 'complete'] as const).map((s) => (
            <button key={s} className={`tab${scope === s ? ' active' : ''}`} onClick={() => setScope(s)}>
              {s === 'active' ? 'Active' : 'Merged'}
            </button>
          ))}
        </div>
        <ErrorBox error={q.error} />
        {q.isLoading ? (
          <Loading />
        ) : byBranch.size === 0 ? (
          <p className="subtle" style={{ marginTop: 16 }}>
            {scope === 'active' ? 'No merge requests are on a train.' : 'No merge requests were merged by a train.'}
          </p>
        ) : (
          [...byBranch].map(([branch, cars]) => (
            <div key={branch} style={{ marginTop: 16 }}>
              <h3 style={{ fontSize: '1rem', marginBottom: 8 }}>
                Target branch <span className="branch-ref">{branch}</span>
              </h3>
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      {scope === 'active' && <th>#</th>}
                      <th>Merge request</th>
                      <th>Pipeline</th>
                      <th>Status</th>
                      <th>Added by</th>
                      <th>{scope === 'active' ? 'Added' : 'Merged'}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {cars.map((car, i) => (
                      <tr key={car.id}>
                        {scope === 'active' && <td>{i + 1}</td>}
                        <td>
                          <Link to={`${p}/-/merge_requests/${car.merge_request.iid}`}>!{car.merge_request.iid}</Link> {car.merge_request.title}
                        </td>
                        <td>
                          {car.pipeline ? (
                            <span className="row" style={{ display: 'inline-flex' }}>
                              <CiStatus pipeline={car.pipeline} /> #{car.pipeline.id}
                            </span>
                          ) : (
                            <span className="subtle">None</span>
                          )}
                        </td>
                        <td>{car.status.replace(/_/g, ' ')}</td>
                        <td>{car.user && <UserAvatar user={car.user} size={20} />}</td>
                        <td>
                          <TimeAgo at={scope === 'active' ? car.created_at : car.merged_at} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          ))
        )}
      </EeOnly>
    </>
  );
}
