// A small set of 16px line icons in the spirit of GitLab's.
import logoUrl from '../assets/logo.png';

const paths: Record<string, string> = {
  project: 'M2.5 3.5h4l1.5 1.5h5.5v7.5h-11z',
  group: 'M5.5 7a2 2 0 1 0 0-4 2 2 0 0 0 0 4zM1.5 13c0-2.2 1.8-4 4-4s4 1.8 4 4M11 7.5a1.75 1.75 0 1 0 0-3.5M11.5 9.2c1.7.3 3 1.8 3 3.8',
  home: 'M2 7.5 8 2.5l6 5M3.5 6.5v7h9v-7',
  code: 'M5.5 4 1.5 8l4 4M10.5 4l4 4-4 4',
  file: 'M4 1.5h5l3.5 3.5v9.5H4zM9 1.5V5h3.5',
  folder: 'M1.5 3.5h4.5l1.5 1.5h7v8.5h-13z',
  branch: 'M4.5 2v12M4.5 11.5c0-4 7-2.5 7-7M11.5 2.5a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3zM4.5 11.5a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3z',
  commit: 'M8 5.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5zM1 8h4.5M10.5 8H15',
  tag: 'M1.5 1.5h6l7 7-6 6-7-7zM5 4.2a.8.8 0 1 1 0 1.6.8.8 0 0 1 0-1.6z',
  mr: 'M4 2.5v11M4 2.5a1.5 1.5 0 1 1 0 .01M12 13.5V6c0-1.4-1.1-2.5-2.5-2.5H7M8.5 1.5l-2 2 2 2M12 13.5a1.5 1.5 0 1 1 0 .01M4 13.5a1.5 1.5 0 1 1 0 .01',
  settings: 'M8 5.5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5zM8 1v2M8 13v2M1 8h2M13 8h2M3 3l1.5 1.5M11.5 11.5 13 13M13 3l-1.5 1.5M4.5 11.5 3 13',
  users: 'M6 7a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5zM1.5 14c0-2.5 2-4.5 4.5-4.5s4.5 2 4.5 4.5M11 2.2a2.5 2.5 0 0 1 0 4.6M12.5 9.7c1.2.6 2 1.9 2 3.3',
  label: 'M1.5 3.5v4l7 7 6-6-7-7h-4.5zM4.5 4.5h.01',
  milestone: 'M8 1.5v13M8 2.5h5l1.5 2-1.5 2H8M8 8H3.5L2 10l1.5 2H8',
  hook: 'M5 9.5a3 3 0 1 1 3-5.2M11 6.5a3 3 0 1 1-1.5 5.6M5 12.5a3 3 0 1 1 3.5-4.5',
  token: 'M10.5 1.5a4 4 0 1 1-3.6 5.7L1.5 12.5v2h3v-1.5H6v-1.5h1.5l1.4-1.4A4 4 0 0 1 10.5 1.5zM11 4.5h.01',
  admin: 'M8 1.5 2.5 3.5v4c0 3.3 2.3 6.1 5.5 7 3.2-.9 5.5-3.7 5.5-7v-4z',
  plus: 'M8 2.5v11M2.5 8h11',
  check: 'm3 8.5 3 3 7-7',
  close: 'm3.5 3.5 9 9M12.5 3.5l-9 9',
  chevronDown: 'm4 6 4 4 4-4',
  chevronRight: 'm6 4 4 4-4 4',
  search: 'M7 2.5a4.5 4.5 0 1 1 0 9 4.5 4.5 0 0 1 0-9zM10.5 10.5l3.5 3.5',
  comment: 'M2 3h12v8H6l-3 2.5V11H2z',
  todo: 'M2.5 2.5h11v11h-11zM5 8l2 2 4-4',
  issues: 'M8 1.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13zM8 5v4M8 11h.01',
  copy: 'M5.5 5.5h8v8h-8zM2.5 10.5v-8h8',
  edit: 'M10.5 2.5l3 3-8 8h-3v-3z',
  trash: 'M2.5 4h11M6 4V2.5h4V4M4 4l.8 9.5h6.4L12 4',
  smile: 'M8 1.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13zM5.5 9.5c.6 1 1.4 1.5 2.5 1.5s1.9-.5 2.5-1.5M6 6.2h.01M10 6.2h.01',
  thumbsUp: 'M4.5 7v7h-3V7zM4.5 7.5 7 2c1.2 0 2 .8 2 2v2h4.2c.8 0 1.4.8 1.2 1.6l-1.2 5.2c-.1.7-.7 1.2-1.4 1.2H4.5',
  thumbsDown: 'M4.5 9V2h-3v7zM4.5 8.5 7 14c1.2 0 2-.8 2-2v-2h4.2c.8 0 1.4-.8 1.2-1.6L13.2 3.2c-.1-.7-.7-1.2-1.4-1.2H4.5',
  approve: 'M8 1.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13zM5 8.2l2 2 4-4',
  external: 'M9.5 2.5h4v4M13.5 2.5 7 9M11.5 9.5v4h-9v-9h4',
  logout: 'M6 2.5H2.5v11H6M10.5 5l3 3-3 3M13.5 8H6',
  pipeline: 'M3 8a1.5 1.5 0 1 1 0 .01M8 8a1.5 1.5 0 1 1 0 .01M13 8a1.5 1.5 0 1 1 0 .01M4.5 8h2M9.5 8h2',
  activity: 'M1.5 8.5h3l2-5 3 9 2-4h3',
  doc: 'M3.5 1.5h9v13h-9zM6 5h4M6 8h4M6 11h2.5',
  lock: 'M4 7.5h8v6.5H4zM5.5 7.5V5a2.5 2.5 0 0 1 5 0v2.5',
  eye: 'M1 8s2.5-5 7-5 7 5 7 5-2.5 5-7 5-7-5-7-5zM8 6a2 2 0 1 1 0 4 2 2 0 0 1 0-4z',
  resolve: 'M8 1.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13zM5 8l2 2 4-4',
  reply: 'M6.5 4 2.5 8l4 4M2.5 8H10c2 0 3.5 1.5 3.5 3.5v1',
  menu: 'M2 4h12M2 8h12M2 12h12',
  history: 'M2.5 8a5.5 5.5 0 1 0 1.6-3.9M2.5 2.5v2.5H5M8 5v3l2 1.5',
  upload: 'M8 11V2.5M4.5 6 8 2.5 11.5 6M2.5 11v2.5h11V11',
  rocket: 'M9.5 2.5c2-1 4-1 4-1s0 2-1 4l-4 4-3-3zM6.5 6.5 4 6.5 2 8.5l2.5.5M9.5 9.5v2.5l-2 2-.5-2.5M4 12c-1 0-1.5 1-1.5 1.5S3 14 4 14s1.5-.5 2-1',
  ellipsis: 'M3.5 8h.01M8 8h.01M12.5 8h.01',
  expand: 'M2.5 6V2.5H6M10 2.5h3.5V6M13.5 10v3.5H10M6 13.5H2.5V10',
};

export type IconName = keyof typeof paths;

export function Icon({ name, size = 16, className, title }: { name: IconName | string; size?: number; className?: string; title?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={name === 'ellipsis' ? 2.5 : 1.4}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden={title ? undefined : true}
      role={title ? 'img' : undefined}
    >
      {title && <title>{title}</title>}
      <path d={paths[name] ?? paths.file} />
    </svg>
  );
}

/** The labrat mark. */
export function Logo({ size = 24 }: { size?: number }) {
  return <img src={logoUrl} width={size} height={size} alt="" aria-hidden />;
}
