<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { loadSession, session, signOut } from '$lib/session.svelte';

  interface SessionRow {
    id: string;
    created_at: string;
    expires_at: string;
  }

  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let sessions = $state<SessionRow[]>([]);
  let form = $state({
    subtitle_mode: 'auto',
    subtitle_lang: '',
    audio_lang: '',
    remote_cap: '',
  });

  onMount(async () => {
    await loadSession();
    const p = session.me?.active_profile;
    if (p) {
      form = {
        subtitle_mode: p.subtitle_mode,
        subtitle_lang: p.subtitle_lang ?? '',
        audio_lang: p.audio_lang ?? '',
        remote_cap: p.remote_bitrate_cap_kbps ? String(p.remote_bitrate_cap_kbps) : '',
      };
    }
    const { data } = await api<SessionRow[]>('/api/v1/auth/sessions');
    sessions = data ?? [];
  });

  async function save(event: SubmitEvent) {
    event.preventDefault();
    const p = session.me?.active_profile;
    if (!p) return;
    const body: Record<string, unknown> = {
      subtitle_mode: form.subtitle_mode,
      subtitle_lang: form.subtitle_lang.trim() || null,
      audio_lang: form.audio_lang.trim() || null,
    };
    if (form.remote_cap.trim()) {
      body.remote_bitrate_cap_kbps = parseInt(form.remote_cap, 10);
    } else {
      body.clear_bitrate_cap = true;
    }
    const { status, error: err } = await api(`/api/v1/profiles/${p.id}`, {
      method: 'PATCH',
      body: JSON.stringify(body),
    });
    if (status === 200) {
      notice = 'Saved.';
      await loadSession();
    } else {
      error = err ?? 'Could not save settings.';
    }
  }

  async function revoke(id: string) {
    const { status } = await api(`/api/v1/auth/sessions/${id}`, { method: 'DELETE' });
    if (status === 204) {
      sessions = sessions.filter((s) => s.id !== id);
      notice = 'Session revoked.';
    }
  }

  async function logout() {
    await signOut();
    await goto('/login');
  }
</script>

<h1>Settings</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if notice}<p class="notice" role="status">{notice}</p>{/if}

{#if session.me?.active_profile}
  <h2>Playback — {session.me.active_profile.name}</h2>
  <form onsubmit={save} class="form">
    <div class="field">
      <label for="submode">Subtitles</label>
      <select id="submode" bind:value={form.subtitle_mode}>
        <option value="auto">Auto (forced / language match)</option>
        <option value="always">Always on</option>
        <option value="never">Never</option>
      </select>
    </div>
    <div class="field">
      <label for="sublang">Preferred subtitle language (e.g. en)</label>
      <input id="sublang" bind:value={form.subtitle_lang} maxlength="8" />
    </div>
    <div class="field">
      <label for="audiolang">Preferred audio language (e.g. en)</label>
      <input id="audiolang" bind:value={form.audio_lang} maxlength="8" />
    </div>
    <div class="field">
      <label for="cap">Remote bitrate cap, kbps (blank = no cap)</label>
      <input id="cap" bind:value={form.remote_cap} inputmode="numeric" />
    </div>
    <button class="btn primary" type="submit">Save</button>
  </form>
{:else}
  <p class="dim">Pick a profile first — playback settings live on profiles.</p>
{/if}

<h2>Sessions</h2>
<p class="dim">Devices signed in as {session.me?.user.username}.</p>
{#if sessions.length === 0}
  <p class="dim">No other sessions.</p>
{:else}
  <ul class="sessions">
    {#each sessions as s (s.id)}
      <li>
        <span class="dim">since {new Date(s.created_at).toLocaleString()}</span>
        <button class="btn small danger" onclick={() => revoke(s.id)}>Revoke</button>
      </li>
    {/each}
  </ul>
{/if}

<h2>Account</h2>
<p>
  <a class="btn" href="/profiles">Switch profile</a>
  <button class="btn" onclick={logout}>Sign out</button>
</p>

<style>
  .form {
    max-width: 26rem;
  }
  .sessions {
    list-style: none;
    padding: 0;
    display: grid;
    gap: 0.5rem;
    max-width: 34rem;
  }
  .sessions li {
    display: flex;
    justify-content: space-between;
    align-items: center;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.5rem 0.8rem;
  }
</style>
