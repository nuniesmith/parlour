<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/state';
  import { api } from '$lib/api';

  let status = $state<'checking' | 'open' | 'invalid'>('checking');
  let username = $state('');
  let password = $state('');
  let error = $state<string | null>(null);

  const token = $derived(page.params.token ?? '');

  onMount(async () => {
    const { data } = await api<{ valid: boolean }>(`/api/v1/invites/${token}`);
    status = data?.valid ? 'open' : 'invalid';
  });

  async function accept(event: SubmitEvent) {
    event.preventDefault();
    const { status: code, error: err } = await api('/api/v1/invites/accept', {
      method: 'POST',
      body: JSON.stringify({ token, username, password }),
    });
    if (code === 201) await goto('/login');
    else error = err ?? 'Could not create the account.';
  }
</script>

<main class="wrap">
  <h1>Parlour</h1>
  {#if status === 'checking'}
    <p class="dim">Checking your invite…</p>
  {:else if status === 'open'}
    <p class="dim">You've been invited. Pick a username and password to join.</p>
    <form onsubmit={accept}>
      <div class="field">
        <label for="username">Username</label>
        <input id="username" bind:value={username} autocomplete="username" required />
      </div>
      <div class="field">
        <label for="password">Password (at least 8 characters)</label>
        <input id="password" type="password" bind:value={password} autocomplete="new-password" required />
      </div>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <button class="btn primary" type="submit">Create account</button>
    </form>
  {:else}
    <p>That invite link isn't usable — it may have been used, expired, or mistyped. Ask the admin for a fresh one.</p>
  {/if}
</main>

<style>
  .wrap {
    max-width: 22rem;
    margin: 12vh auto;
    padding: 0 1rem;
  }
  h1 {
    color: var(--accent);
  }
</style>
