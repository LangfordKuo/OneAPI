export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/**
 * Window event fired when the backend rejects a request with 401. The app
 * listens for it, invalidates the `['me']` query and routes back to the login
 * screen. Used by the fetch helpers and by the SSE loop (which cannot import
 * React state).
 */
export const UNAUTHORIZED_EVENT = 'oneapi:unauthorized';

/** Announce an expired session to the app shell. */
export function handleUnauthorized(): void {
  window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
}

/**
 * Only same-origin admin API paths are allowed. Rejecting traversal and
 * protocol-relative forms keeps callers from accidentally turning a data
 * value into an absolute or parent-relative URL.
 */
function assertApiPath(path: string): void {
  if (!path.startsWith('/api/')) {
    throw new Error(`API path must start with /api/: ${path}`);
  }
  if (path.includes('..')) {
    throw new Error(`API path must not contain "..": ${path}`);
  }
  if (path.includes('//')) {
    throw new Error(`API path must not contain "//": ${path}`);
  }
}

function isAuthPath(path: string): boolean {
  return (
    path.startsWith('/api/auth/login') ||
    path.startsWith('/api/auth/me') ||
    path.startsWith('/api/auth/password')
  );
}

/**
 * Perform a request and hand back the raw `Response`, applying the same
 * credentials, error-message extraction and 401 redirect as `request`.
 *
 * Needed by callers that must not parse the body as JSON: SSE streams and
 * database backups.
 */
export async function requestRaw(
  path: string,
  init?: RequestInit,
): Promise<Response> {
  assertApiPath(path);

  const headers = new Headers(init?.headers);
  if (init?.body !== undefined && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  const response = await fetch(path, {
    credentials: 'include',
    ...init,
    headers,
  });

  if (!response.ok) {
    if (response.status === 401 && !isAuthPath(path)) {
      handleUnauthorized();
    }

    let message = response.statusText || `HTTP ${response.status}`;
    try {
      const body = (await response.json()) as { error?: { message?: string } };
      message = body.error?.message ?? message;
    } catch {
      // Response body is not JSON; keep the status text.
    }
    throw new ApiError(response.status, message);
  }

  return response;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  assertApiPath(path);
  const response = await requestRaw(path, init);

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

export const api = {
  get: <T>(path: string) => request<T>(path),
  post: <T>(path: string, body?: unknown) =>
    request<T>(path, {
      method: 'POST',
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
  put: <T>(path: string, body?: unknown) =>
    request<T>(path, {
      method: 'PUT',
      body: body === undefined ? undefined : JSON.stringify(body),
    }),
  delete: <T>(path: string) => request<T>(path, { method: 'DELETE' }),
};
