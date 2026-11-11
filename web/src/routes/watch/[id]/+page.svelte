<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { api } from '$lib/api';
  import { formatBytes, formatDurationMs, type ItemDetail } from '$lib/types';

  let detail = $state<ItemDetail | null>(null);

  onMount(async () => {
    const { data } = await api<ItemDetail>(`/api/v1/items/${page.params.id}`);
    detail = data;
  });
</script>

{#if !detail}
  <p class="dim">Loading…</p>
{:else}
  <h1>{detail.title}</h1>
  <div class="player-stub" role="note">
    <p class="big">▶</p>
    <p>
      Playback arrives in <strong>Milestone 1.6</strong> — direct play first, with resume positions
      and watch state. The file is ready and waiting:
    </p>
    {#each detail.files as file (file.id)}
      <p class="dim">
        {file.relative_path} · {file.container ?? ''} · {formatBytes(file.size_bytes)} · {formatDurationMs(
          file.duration_ms,
        )}
      </p>
    {/each}
    <p><a class="btn" href={`/item/${detail.id}`}>← Back to details</a></p>
  </div>
{/if}

<style>
  .player-stub {
    background: #000;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 2.4rem 1.6rem;
    text-align: center;
  }
  .big {
    font-size: 3rem;
    color: var(--accent);
    margin: 0 0 0.6rem;
  }
</style>
