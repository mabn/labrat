import { Link, useNavigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { api, enc } from '../../api/client';
import type { RepoFile, TreeEntry } from '../../api/types';
import { Avatar, CopyButton, Dropdown, Markdown } from '../../components/ui';
import { Icon } from '../../components/Icon';
import { useProject, useProjectCrumbs } from './ProjectRoutes';
import { EmptyRepo, LastCommit, TreeView } from './Files';
import { decodeContent, RefSelector, useRefs } from './refs';

export function Overview() {
  const { project, canWrite } = useProject();
  const nav = useNavigate();
  useProjectCrumbs([]);
  const refs = useRefs(project.id);
  const ref_ = project.default_branch ?? 'main';
  const tree = useQuery({
    queryKey: ['tree', project.id, ref_, ''],
    queryFn: () => api.all<TreeEntry>(`/projects/${project.id}/repository/tree`, { ref: ref_ }, 5),
    enabled: !project.empty_repo,
  });
  const readmeEntry = tree.data?.find((e) => e.type === 'blob' && /^readme(\.(md|markdown|txt))?$/i.test(e.name));
  const readme = useQuery({
    queryKey: ['file', project.id, ref_, readmeEntry?.path],
    queryFn: () => api.get<RepoFile>(`/projects/${project.id}/repository/files/${enc(readmeEntry!.path)}`, { ref: ref_ }),
    enabled: !!readmeEntry,
  });
  const p = `/${project.path_with_namespace}/-`;
  return (
    <>
      <div className="page-head" style={{ borderBottom: 0, paddingBottom: 4 }}>
        <Avatar name={project.name} size={48} square />
        <div>
          <h1 className="row" style={{ gap: 8 }}>
            {project.name}
            <span className="subtle" title={`Visibility: ${project.visibility}`}>
              <Icon name={project.visibility === 'private' ? 'lock' : 'eye'} />
            </span>
          </h1>
          <div className="small subtle">Project ID: {project.id}</div>
        </div>
        <div className="actions">
          <Dropdown
            right
            button={(_, toggle) => (
              <button className="btn btn-confirm" onClick={toggle}>
                Code <Icon name="chevronDown" />
              </button>
            )}
          >
            {() => (
              <div style={{ padding: 12, width: 380 }}>
                <div className="label-text">Clone with HTTP</div>
                <div className="clone-box" style={{ marginBottom: 12 }}>
                  <input type="text" readOnly value={project.http_url_to_repo} />
                  <CopyButton text={project.http_url_to_repo} />
                </div>
                <div className="label-text">Clone with SSH</div>
                <div className="clone-box">
                  <input type="text" readOnly value={project.ssh_url_to_repo} />
                  <CopyButton text={project.ssh_url_to_repo} />
                </div>
                <div className="help">labrat serves git over HTTP only.</div>
              </div>
            )}
          </Dropdown>
        </div>
      </div>
      {project.description && <p className="subtle">{project.description}</p>}
      <div className="row wrap small" style={{ gap: 16, margin: '8px 0 16px' }}>
        <Link to={`${p}/commits/${ref_}`} className="row" style={{ gap: 4 }}>
          <Icon name="commit" /> Commits
        </Link>
        <Link to={`${p}/branches`} className="row" style={{ gap: 4 }}>
          <Icon name="branch" /> {refs.data?.branches.length ?? '…'} Branches
        </Link>
        <Link to={`${p}/tags`} className="row" style={{ gap: 4 }}>
          <Icon name="tag" /> {refs.data?.tags.length ?? '…'} Tags
        </Link>
        <Link to={`${p}/merge_requests`} className="row" style={{ gap: 4 }}>
          <Icon name="mr" /> Merge requests
        </Link>
      </div>
      {project.empty_repo ? (
        <EmptyRepo project={project} />
      ) : (
        <>
          <div className="repo-head">
            <RefSelector projectId={project.id} value={ref_} onSelect={(r) => nav(`${p}/tree/${r}`)} />
            <span className="spacer" />
            {canWrite && (
              <Link className="btn" to={`${p}/new/${ref_}`}>
                <Icon name="plus" /> New file
              </Link>
            )}
            <Link className="btn" to={`${p}/merge_requests/new`}>
              Create merge request
            </Link>
          </div>
          <LastCommit project={project} ref_={ref_} path="" />
          <TreeView project={project} ref_={ref_} path="" />
          {readme.data && (
            <div className="card" style={{ marginTop: 16 }}>
              <div className="card-head">
                <Icon name="doc" /> {readme.data.file_name}
              </div>
              <div className="card-body" style={{ padding: 24 }}>
                {/\.(md|markdown)$/i.test(readme.data.file_name) ? (
                  <Markdown source={decodeContent(readme.data.content).text} projectPath={project.path_with_namespace} />
                ) : (
                  <pre style={{ whiteSpace: 'pre-wrap', margin: 0 }}>{decodeContent(readme.data.content).text}</pre>
                )}
              </div>
            </div>
          )}
        </>
      )}
    </>
  );
}
