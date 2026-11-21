/** URL helpers for GitLab-shaped paths. */
export const projectUrl = (fullPath: string, rest = '') => `/${fullPath}${rest ? `/-/${rest}` : ''}`;
export const mrUrl = (fullPath: string, iid: number, tab = '') =>
  `/${fullPath}/-/merge_requests/${iid}${tab ? `/${tab}` : ''}`;
export const encPath = (p: string) => p.split('/').map(encodeURIComponent).join('/');

/** Strips the instance's origin from a `web_url`, so links stay in the app. */
export function localPath(webUrl: string): string {
  try {
    const u = new URL(webUrl);
    return u.pathname + u.search + u.hash;
  } catch {
    return webUrl;
  }
}
