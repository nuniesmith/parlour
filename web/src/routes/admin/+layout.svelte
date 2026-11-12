<script lang="ts">
  // Server section (UI6): every page under /admin shares this
  // guard and the table / form styles. The API is the real
  // gate (admin routes answer 404 to everyone else); this only
  // keeps a non-admin who types the URL from seeing empty shells.
  import { session } from '$lib/session.svelte';

  let { children } = $props();
</script>

{#if !session.loaded}
  <p class="dim">Loading…</p>
{:else if !session.me?.user.is_admin}
  <h2>Page not found</h2>
  <p class="dim">There is nothing here. <a class="link" href="/">Back to Home</a></p>
{:else}
  <div class="admin">
    {@render children()}
  </div>
{/if}

<style>
  .link {
    color: var(--accent);
  }
  .admin :global(table) {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 1.6rem;
  }
  .admin :global(th),
  .admin :global(td) {
    text-align: left;
    padding: 0.5rem 0.6rem;
    border-bottom: 1px solid var(--line);
    vertical-align: top;
  }
  .admin :global(th) {
    color: var(--text-dim);
    font-size: 0.85rem;
  }
  .admin :global(.table-wrap) {
    overflow-x: auto;
  }
  .admin :global(.actions) {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .admin :global(.form) {
    max-width: 30rem;
    margin-bottom: 1.6rem;
  }
  .admin :global(.row) {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
    margin: 0.8rem 0;
  }
  .admin :global(.row select),
  .admin :global(.row input) {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.5rem 0.6rem;
  }
  .admin :global(.mono) {
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
    word-break: break-all;
  }
</style>
