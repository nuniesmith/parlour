// Tiny API helper: same-origin, cookie auth, JSON in/out.
export async function api<T>(
  path: string,
  options: RequestInit = {},
): Promise<{ status: number; data: T | null; error: string | null }> {
  const res = await fetch(path, {
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json', ...(options.headers ?? {}) },
    ...options,
  });
  let data: T | null = null;
  let error: string | null = null;
  try {
    const body = await res.json();
    if (res.ok) data = body as T;
    else error = body?.error?.message ?? `HTTP ${res.status}`;
  } catch {
    if (!res.ok) error = `HTTP ${res.status}`;
  }
  return { status: res.status, data, error };
}
