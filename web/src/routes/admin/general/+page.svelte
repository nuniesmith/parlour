<script lang="ts">
  // Server → General: how this server is configured. Read-only:
  // every value comes from the server's environment at startup,
  // so the page says which variable sets it instead of offering
  // a form that could not take effect.
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { ServerInfo } from '$lib/types';

  let info = $state<ServerInfo | null>(null);
  let error = $state<string | null>(null);

  const rows = $derived(
    info
      ? [
          { label: 'Server name', value: info.name, env: 'PARLOUR_SERVER_NAME' },
          { label: 'HTTP port', value: String(info.port), env: 'PARLOUR_PORT' },
          { label: 'Home network (LAN) address', value: info.urls.lan, env: 'PARLOUR_URL_LAN' },
          { label: 'Tailnet address', value: info.urls.tailnet, env: 'PARLOUR_URL_TAILNET' },
          { label: 'Public address', value: info.urls.public, env: 'PARLOUR_URL_PUBLIC' },
        ]
      : [],
  );

  onMount(async () => {
    const { data, error: err } = await api<ServerInfo>('/api/v1/admin/server');
    if (data) info = data;
    else error = err ?? 'Could not load server settings.';
  });
</script>

<h2>General</h2>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if info}
  <p class="dim intro">
    Parlour {info.version}. These settings are read from the server's environment when it starts.
    To change one, set its variable (for example in the <code>.env</code> file beside
    <code>docker-compose.yml</code>) and restart the server.
  </p>
  <div class="table-wrap">
    <table>
      <thead><tr><th>Setting</th><th>Current value</th><th>Set with</th></tr></thead>
      <tbody>
        {#each rows as r (r.env)}
          <tr>
            <td>{r.label}</td>
            <td>{#if r.value}{r.value}{:else}<span class="dim">Not set</span>{/if}</td>
            <td><code>{r.env}</code></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{:else if !error}
  <p class="dim">Loading…</p>
{/if}

<style>
  .intro {
    max-width: 44rem;
  }
  code {
    font-size: 0.85em;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.3rem;
    padding: 0.05rem 0.3rem;
  }
</style>
