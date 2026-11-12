<script lang="ts">
  // Server → Library → Emby import (Milestone 1.7).
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { Job } from '$lib/types';

  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);

  interface EmbyUserOption { id: string; name: string }
  interface ProfileOption { id: string; name: string; username: string }
  interface ImportReport {
    dry_run: boolean;
    emby_items_with_state: number;
    matched: number;
    applied: number;
    unmatched: number;
    ambiguous: number;
    skipped_no_state: number;
    unmatched_items: string[];
    ambiguous_items: string[];
  }
  let embyStatus = $state<{ configured: boolean; reachable: boolean; error: string | null; users: EmbyUserOption[] } | null>(null);
  let embyProfiles = $state<ProfileOption[]>([]);
  let embyUserId = $state('');
  let embyProfileId = $state('');
  let embyReport = $state<ImportReport | null>(null);
  let embyBusy = $state(false);

  async function loadEmby() {
    const [st, pr] = await Promise.all([
      api('/api/v1/admin/emby/status'),
      api('/api/v1/admin/emby/profiles'),
    ]);
    if (st.data) {
      embyStatus = st.data as typeof embyStatus;
      if (!embyUserId && embyStatus?.users.length) embyUserId = embyStatus.users[0].id;
    }
    if (pr.data) {
      embyProfiles = pr.data as ProfileOption[];
      if (!embyProfileId && embyProfiles.length) embyProfileId = embyProfiles[0].id;
    }
    const j = await api('/api/v1/admin/jobs?limit=30');
    const list = (j.data as unknown as Job[] | null) ?? [];
    const last = list.find((x) => x.kind === 'import_emby' && x.result);
    if (last) embyReport = last.result as unknown as ImportReport;
  }

  async function runEmbyImport(dryRun: boolean) {
    embyBusy = true;
    error = null;
    const { status, error: err } = await api('/api/v1/admin/emby/import', {
      method: 'POST',
      body: JSON.stringify({ emby_user_id: embyUserId, profile_id: embyProfileId, dry_run: dryRun }),
    });
    embyBusy = false;
    if (err) {
      error = err;
      return;
    }
    notice = dryRun ? 'Dry run queued — the report lands here in a few seconds.' : 'Import queued — watch the report below.';
    // Poll the jobs feed for the finished import's report.
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => setTimeout(r, 2000));
      const j = await api('/api/v1/admin/jobs?limit=30');
      const list = (j.data as unknown as Job[] | null) ?? [];
      const done = list.find((x) => x.kind === 'import_emby' && x.status === 'succeeded' && x.result);
      if (done) {
        embyReport = done.result as unknown as ImportReport;
        break;
      }
      const failed = list.find((x) => x.kind === 'import_emby' && x.status === 'failed');
      if (failed) {
        error = 'The import job failed — see Jobs.';
        break;
      }
    }
  }

  onMount(loadEmby);
</script>

<p><a class="link" href="/admin/library">← Library</a></p>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

<h2>Emby import</h2>
<p class="dim">
  Carry watched state, resume positions and favourites over from Emby.
  Matching is by TMDB/TVDB/IMDb id, never by title. Run a dry run first —
  it writes nothing and shows exactly what would land.
</p>
{#if !embyStatus}
  <p class="dim">Checking the Emby connection…</p>
{:else if !embyStatus.configured}
  <p class="error">
    Emby is not configured on the server. Set PARLOUR_EMBY_URL and
    PARLOUR_EMBY_API_KEY in the server environment and restart.
  </p>
{:else if !embyStatus.reachable}
  <p class="error">Emby is configured but unreachable: {embyStatus.error}</p>
{:else}
  <div class="row">
    <label>Emby user
      <select bind:value={embyUserId}>
        {#each embyStatus.users as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
      </select>
    </label>
    <span aria-hidden="true">→</span>
    <label>Parlour profile
      <select bind:value={embyProfileId}>
        {#each embyProfiles as p (p.id)}<option value={p.id}>{p.username} / {p.name}</option>{/each}
      </select>
    </label>
  </div>
  <div class="row">
    <button class="btn" disabled={embyBusy} onclick={() => runEmbyImport(true)}>Dry run (writes nothing)</button>
    <button class="btn primary" disabled={embyBusy} onclick={() => runEmbyImport(false)}>Import for real</button>
  </div>
  {#if embyReport}
    <h3>Last report{embyReport.dry_run ? ' (dry run)' : ''}</h3>
    <ul>
      <li>{embyReport.emby_items_with_state} Emby items had state worth carrying</li>
      <li>{embyReport.matched} matched{embyReport.dry_run ? '' : `, ${embyReport.applied} item states written`}</li>
      <li>{embyReport.unmatched} unmatched, {embyReport.ambiguous} ambiguous, {embyReport.skipped_no_state} skipped (no state)</li>
    </ul>
    {#if embyReport.unmatched_items.length}
      <h4>Unmatched — nothing was imported for these</h4>
      <ul>{#each embyReport.unmatched_items as name (name)}<li>{name}</li>{/each}</ul>
    {/if}
    {#if embyReport.ambiguous_items.length}
      <h4>Ambiguous — the id matched more than one title; skipped on purpose</h4>
      <ul>{#each embyReport.ambiguous_items as name (name)}<li>{name}</li>{/each}</ul>
    {/if}
  {/if}
{/if}

<style>
  .link {
    color: var(--accent);
  }
</style>
