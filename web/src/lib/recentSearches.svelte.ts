// Recent searches for the active profile (UI9). Stored server-side
// on the profile (`/api/v1/search/recent`), so they follow it to
// every device. Shared by the sidebar search box (offered as
// suggestions) and the Search page (listed while the field is
// empty). A search is recorded only when it is committed — Enter,
// or opening a result — never per keystroke.

import { api } from './api';
import { session } from './session.svelte';

export interface RecentSearch {
  id: string;
  query: string;
  searched_at: string;
}

export const recent = $state<{ profileId: string | null; items: RecentSearch[] }>({
  profileId: null,
  items: [],
});

const BASE = '/api/v1/search/recent';

function activeProfileId(): string | null {
  return session.me?.active_profile?.id ?? null;
}

/** The query as the server would store it, or null if too short. */
export function committable(query: string): string | null {
  const q = query.trim();
  return q.length >= 2 ? q : null;
}

/** Fetch the active profile's list (cheap; called on focus / page load). */
export async function loadRecent(): Promise<void> {
  const profileId = activeProfileId();
  if (!profileId) {
    recent.profileId = null;
    recent.items = [];
    return;
  }
  const { data } = await api<RecentSearch[]>(BASE);
  // The profile may have changed while this was in flight.
  if (activeProfileId() !== profileId) return;
  recent.profileId = profileId;
  recent.items = data ?? [];
}

/** Remember a committed search. Failures are silent: it's a nicety. */
export async function recordSearch(query: string): Promise<void> {
  const q = committable(query);
  const profileId = activeProfileId();
  if (!q || !profileId) return;
  const { data } = await api<RecentSearch[]>(BASE, {
    method: 'POST',
    body: JSON.stringify({ query: q }),
  });
  if (data && activeProfileId() === profileId) {
    recent.profileId = profileId;
    recent.items = data;
  }
}

export async function forgetSearch(id: string): Promise<void> {
  const { status } = await api(`${BASE}/${encodeURIComponent(id)}`, { method: 'DELETE' });
  if (status === 204 || status === 404) {
    recent.items = recent.items.filter((r) => r.id !== id);
  }
}

export async function clearSearches(): Promise<void> {
  const { status } = await api(BASE, { method: 'DELETE' });
  if (status === 204) recent.items = [];
}
