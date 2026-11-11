<script lang="ts">
  import { goto } from '$app/navigation';
  import { api } from '$lib/api';

  let username = '';
  let password = '';
  let error: string | null = null;
  let busy = false;

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
      await goto('/');
    } else {
      error = err ?? 'Sign-in failed.';
    }
  }
</script>

<main>
  <h1>Parlour</h1>
  <p class="sub">Sign in to your server.</p>
  <form onsubmit={submit}>
    <label>
      Username
      <input bind:value={username} autocomplete="username" required />
    </label>
    <label>
      Password
      <input
        bind:value={password}
        type="password"
        autocomplete="current-password"
        required
      />
    </label>
    {#if error}<p class="error">{error}</p>{/if}
    <button type="submit" disabled={busy}>Sign in</button>
  </form>
  <p class="hint">Accounts are invite-only — ask the server owner for an invite link.</p>
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    margin: 4rem auto;
    max-width: 22rem;
    padding: 0 1rem;
  }
  form {
    display: grid;
    gap: 0.9rem;
  }
  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.9rem;
  }
  input {
    padding: 0.55rem 0.7rem;
    border: 1px solid #bbb;
    border-radius: 0.5rem;
    font-size: 1rem;
  }
  button {
    padding: 0.6rem;
    border: 0;
    border-radius: 0.5rem;
    background: #7c5cbf;
    color: white;
    font-size: 1rem;
    cursor: pointer;
  }
  .sub {
    color: #666;
  }
  .hint {
    color: #888;
    font-size: 0.85rem;
  }
  .error {
    color: #b3261e;
  }
</style>
