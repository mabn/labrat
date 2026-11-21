import { Link } from 'react-router-dom';
import type { Label, MergeRequest } from '../api/types';
import { Icon } from './Icon';
import { CiStatus, LabelPill, TimeAgo, UserAvatar } from './ui';
import { localPath } from '../lib/paths';

export function MrList({ mrs, showProject }: { mrs: MergeRequest[]; showProject?: boolean }) {
  return (
    <ul className="list list-bordered issuable-list">
      {mrs.map((mr) => (
        <li key={mr.id}>
          <div className="grow">
            <Link to={localPath(mr.web_url)} className="issuable-title">
              {mr.draft && <span className="badge badge-neutral" style={{ marginRight: 6 }}>Draft</span>}
              {mr.title}
            </Link>
            <div className="issuable-meta">
              <span>{showProject ? mr.references.full : mr.references.short}</span>
              <span>·</span>
              <span>
                created <TimeAgo at={mr.created_at} /> by {mr.author.name}
              </span>
              <span className="branch-ref" title="Target branch">
                <Icon name="branch" size={12} /> {mr.target_branch}
              </span>
              {mr.milestone && (
                <span className="row" style={{ gap: 2 }}>
                  <Icon name="milestone" size={12} /> {mr.milestone.title}
                </span>
              )}
              {(mr.labels as (string | Label)[]).map((l) =>
                typeof l === 'string' ? <LabelPill key={l} name={l} /> : <LabelPill key={l.id} label={l} />,
              )}
            </div>
          </div>
          <div className="issuable-side">
            <div className="row" style={{ gap: 10 }}>
              {mr.state === 'merged' && <span className="badge badge-info">Merged</span>}
              {mr.state === 'closed' && <span className="badge badge-danger">Closed</span>}
              <CiStatus pipeline={(mr as MergeRequest & { head_pipeline?: MergeRequest['head_pipeline'] }).head_pipeline} />
              {mr.assignees.length > 0 && (
                <span className="avatar-stack" title="Assignees">
                  {mr.assignees.slice(0, 3).map((u) => (
                    <UserAvatar key={u.id} user={u} size={18} />
                  ))}
                </span>
              )}
              {mr.upvotes > 0 && (
                <span className="row" style={{ gap: 2 }}>
                  <Icon name="thumbsUp" size={14} /> {mr.upvotes}
                </span>
              )}
              <span className="row" style={{ gap: 2 }} title="Comments">
                <Icon name="comment" size={14} /> {mr.user_notes_count}
              </span>
            </div>
            <span>
              updated <TimeAgo at={mr.updated_at} />
            </span>
          </div>
        </li>
      ))}
    </ul>
  );
}
