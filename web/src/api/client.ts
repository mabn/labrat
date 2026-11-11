// A small client for labrat's GitLab-compatible REST API (`/api/v4`) and GraphQL API.
// Authentication is either the web session (cookie + CSRF token, as GitLab's own frontend does)
// or a personal access token kept in this browser.

const TOKEN_KEY = 'labrat.token';

export function storedToken(): string | null {
  try {
    return localStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

export function setStoredToken(token: string | null) {
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    /* storage unavailable */
  }
}

export function csrfToken(): string | null {
  const m = document.cookie.match(/(?:^|;\s*)_labrat_csrf=([^;]+)/);
  return m ? decodeURIComponent(m[1]) : null;
}

export class ApiError extends Error {
  status: number;
  body: unknown;
  constructor(status: number, body: unknown) {
    super(errorMessage(status, body));
    this.status = status;
    this.body = body;
  }
}

function errorMessage(status: number, body: unknown): string {
  if (body && typeof body === 'object') {
    const b = body as Record<string, unknown>;
    const m = b.message ?? b.error;
    if (typeof m === 'string') return m;
    if (Array.isArray(m)) return m.join(', ');
    if (m && typeof m === 'object') {
      return Object.entries(m as Record<string, unknown>)
        .map(([k, v]) => `${k} ${Array.isArray(v) ? v.join(', ') : String(v)}`)
        .join('; ');
    }
  }
  if (typeof body === 'string' && body) return body;
  return `Request failed (${status})`;
}

type Query = Record<string, string | number | boolean | undefined | null | (string | number)[]>;

export function qs(query?: Query): string {
  if (!query) return '';
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v === undefined || v === null || v === '') continue;
    if (Array.isArray(v)) v.forEach((x) => p.append(`${k}[]`, String(x)));
    else p.append(k, String(v));
  }
  const s = p.toString();
  return s ? `?${s}` : '';
}

export interface Page<T> {
  items: T[];
  page: number;
  totalPages: number | null;
  total: number | null;
  nextPage: number | null;
}

function headers(method: string, json: boolean): HeadersInit {
  // Marks the app's own calls, which labrat keeps out of its request log.
  const h: Record<string, string> = { Accept: 'application/json', 'X-Labrat-UI': '1' };
  if (json) h['Content-Type'] = 'application/json';
  const token = storedToken();
  if (token) h['PRIVATE-TOKEN'] = token;
  else if (method !== 'GET' && method !== 'HEAD') {
    const csrf = csrfToken();
    if (csrf) h['X-CSRF-Token'] = csrf;
  }
  return h;
}

async function send(method: string, path: string, body?: unknown, query?: Query): Promise<Response> {
  const isForm = body instanceof FormData;
  const res = await fetch(`/api/v4${path}${qs(query)}`, {
    method,
    headers: headers(method, body !== undefined && !isForm),
    body: body === undefined ? undefined : isForm ? body : JSON.stringify(body),
    credentials: 'same-origin',
  });
  if (!res.ok) {
    const text = await res.text();
    let parsed: unknown = text;
    try {
      parsed = JSON.parse(text);
    } catch {
      /* not JSON */
    }
    throw new ApiError(res.status, parsed);
  }
  return res;
}

async function parse<T>(res: Response): Promise<T> {
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  if (!text) return undefined as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    return text as T;
  }
}

export const api = {
  get: async <T>(path: string, query?: Query) => parse<T>(await send('GET', path, undefined, query)),
  text: async (path: string, query?: Query) => (await send('GET', path, undefined, query)).text(),
  post: async <T>(path: string, body?: unknown, query?: Query) => parse<T>(await send('POST', path, body ?? {}, query)),
  put: async <T>(path: string, body?: unknown, query?: Query) => parse<T>(await send('PUT', path, body ?? {}, query)),
  patch: async <T>(path: string, body?: unknown) => parse<T>(await send('PATCH', path, body ?? {})),
  del: async <T = void>(path: string, query?: Query) => parse<T>(await send('DELETE', path, undefined, query)),

  /** One page, with GitLab's pagination headers. */
  page: async <T>(path: string, query?: Query): Promise<Page<T>> => {
    const res = await send('GET', path, undefined, query);
    const num = (h: string) => {
      const v = res.headers.get(h);
      return v ? Number(v) : null;
    };
    return {
      items: await parse<T[]>(res),
      page: num('x-page') ?? 1,
      totalPages: num('x-total-pages'),
      total: num('x-total'),
      nextPage: num('x-next-page'),
    };
  },

  /** Every page of a list (capped, to stay polite). */
  all: async <T>(path: string, query?: Query, maxPages = 20): Promise<T[]> => {
    const out: T[] = [];
    let page = 1;
    for (let i = 0; i < maxPages; i++) {
      const p = await api.page<T>(path, { per_page: 100, ...query, page });
      out.push(...p.items);
      if (!p.nextPage) break;
      page = p.nextPage;
    }
    return out;
  },

  graphql: async <T>(query: string, variables?: Record<string, unknown>): Promise<T> => {
    const res = await fetch('/api/graphql', {
      method: 'POST',
      headers: headers('POST', true),
      body: JSON.stringify({ query, variables }),
      credentials: 'same-origin',
    });
    const body = await res.json();
    if (body.errors?.length) throw new ApiError(res.status, { message: body.errors.map((e: { message: string }) => e.message).join('; ') });
    return body.data as T;
  },
};

/** `/projects/:id` path segment for a numeric id or a full path. */
export const enc = (idOrPath: string | number) => encodeURIComponent(String(idOrPath));
