<script lang="ts">
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';
  import { loadSession } from '$lib/session.svelte';

  let username = $state('');
  let password = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = null;
    const { status, error: err } = await api('/api/v1/auth/login', {
      method: 'POST',
      body: JSON.stringify({ username, password }),
    });
    busy = false;
    if (status === 200) {
      const state = await loadSession();
      await goto(state === 'ready' ? '/' : '/profiles');
    } else if (status === 429) {
      error = 'Too many attempts — wait a minute and try again.';
    } else {
      error = err ?? 'Sign-in failed.';
    }
  }
</script>

<main class="wrap">
  <h1>Parlour</h1>
  <p class="dim">Sign in to your media parlour.</p>
  <form onsubmit={submit}>
    <div class="field">
      <label for="username">Username</label>
      <input id="username" bind:value={username} autocomplete="username" required />
    </div>
    <div class="field">
      <label for="password">Password</label>
      <input
        id="password"
        type="password"
        bind:value={password}
        autocomplete="current-password"
        required
      />
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <button class="btn primary" type="submit" disabled={busy}>Sign in</button>
  </form>
  <p class="dim small">No account? Parlour is invite-only — ask the admin for an invite link.</p>
</main>

<style>
  .wrap {
    max-width: 22rem;
    margin: 12vh auto;
    padding: 0 1rem;
  }
  h1 {
    color: var(--accent);
    margin-bottom: 0.2rem;
  }
  .small {
    font-size: 0.85rem;
    margin-top: 1.2rem;
  }
</style>
