<script lang="ts">
  // Alerts rail (UI5): the server's active notices, newest
  // first, each with a severity icon, a timestamp, a link to
  // where it can be dealt with, and a per-admin Dismiss.
  import type { Notice } from '$lib/types';

  let {
    notices,
    ondismiss,
  }: { notices: Notice[] | null; ondismiss: (n: Notice) => void } = $props();

  const LINKS: Record<string, { href: string; label: string }> = {
    job_failed: { href: '/admin/jobs', label: 'Open jobs' },
    metadata_review: { href: '/admin/metadata', label: 'Review matches' },
  };

  const SEVERITY_LABEL: Record<string, string> = {
    info: 'For your information',
    warning: 'Warning',
    error: 'Problem',
  };

  function when(iso: string): string {
    return new Date(iso).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  }

  function extra(n: Notice): string | null {
    const error = n.detail?.error;
    return typeof error === 'string' && error ? error : null;
  }
</script>

<aside class="rail" aria-label="Alerts">
  <h2>Alerts</h2>
  {#if notices === null}
    <p class="dim">Loading…</p>
  {:else if notices.length === 0}
    <p class="dim">Nothing needs your attention.</p>
  {:else}
    <ul>
      {#each notices as n (n.id)}
        <li class={`sev-${n.severity}`}>
          <span class="icon" role="img" aria-label={SEVERITY_LABEL[n.severity] ?? n.severity}>
            <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
              {#if n.severity === 'error'}
                <circle cx="12" cy="12" r="10" />
                <path d="M8 8l8 8M16 8l-8 8" />
              {:else if n.severity === 'warning'}
                <path d="M12 3L2.5 20h19z" />
                <path d="M12 10v4.5M12 17.2v.3" />
              {:else}
                <circle cx="12" cy="12" r="10" />
                <path d="M12 11v6M12 7.2v.3" />
              {/if}
            </svg>
          </span>
          <div class="body">
            <p class="summary">{n.summary}</p>
            {#if extra(n)}<p class="extra dim">{extra(n)}</p>{/if}
            <p class="meta dim">
              <time datetime={n.created_at}>{when(n.created_at)}</time>
            </p>
            <p class="actions">
              {#if LINKS[n.kind]}
                <a href={LINKS[n.kind].href}>{LINKS[n.kind].label}</a>
              {/if}
              <button type="button" onclick={() => ondismiss(n)}>Dismiss</button>
            </p>
          </div>
        </li>
      {/each}
    </ul>
  {/if}
</aside>

<style>
  .rail {
    min-width: 0;
  }
  h2 {
    margin: 0 0 0.7rem;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.6rem;
  }
  li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 0.6rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    padding: 0.7rem 0.8rem;
  }
  .icon {
    display: grid;
    color: var(--text-dim);
  }
  svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 2;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .sev-warning .icon {
    color: var(--accent);
  }
  .sev-error .icon {
    color: var(--danger);
  }
  p {
    margin: 0;
  }
  .summary {
    overflow-wrap: anywhere;
  }
  .extra,
  .meta {
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.9rem;
    margin-top: 0.3rem;
    font-size: 0.9rem;
  }
  .actions a,
  .actions button {
    background: none;
    border: 0;
    padding: 0.2rem 0;
    cursor: pointer;
    color: var(--accent);
  }
  .actions button {
    color: var(--text-dim);
  }
  .actions button:hover {
    color: var(--text);
  }
</style>
