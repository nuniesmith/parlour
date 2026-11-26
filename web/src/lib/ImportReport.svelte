<script lang="ts">
  // XFER8: renders XFER3's import report (`import::Report`) — the dry
  // run before an import is applied, and the apply job's result after.
  // The two lines an admin must see before applying sit at the top:
  // D38 (the bundle's admin takes over this server's untouched first
  // admin) and D37 (a bundle without hashes creates accounts nobody can
  // sign in to until an admin sets a password).
  import { formatDateTime, type ImportReport, type ImportTally } from '$lib/types';

  let {
    report,
    folders = $bindable<Record<string, string> | undefined>(undefined),
  }: {
    report: ImportReport;
    /** Present while the admin can still change the import: the
     *  replacement folders per library name, one per line. */
    folders?: Record<string, string>;
  } = $props();

  const will = $derived(report.dry_run);

  const adopted = $derived(report.users.filter((u) => u.action === 'adopt'));
  const locked = $derived(report.users.filter((u) => u.sign_in === 'locked'));
  const userConflicts = $derived(report.users.filter((u) => u.action === 'conflict'));

  const USER_ACTION: Record<string, string> = {
    create: 'New account',
    existing: 'Already here',
    adopt: 'Takes over the first admin',
    conflict: 'Conflict — skipped',
  };
  const SIGN_IN: Record<string, string> = {
    from_bundle: 'With the old server’s password',
    locked: 'No password until an admin sets one',
    kept: 'Unchanged',
    none: 'Not imported',
  };
  const LIBRARY_ACTION: Record<string, string> = {
    create: 'New library',
    existing: 'Already here',
    conflict: 'Conflict — skipped',
  };

  const tallies = $derived<[string, ImportTally][]>([
    ['Profiles', report.profiles],
    ['Library access', report.access],
    ['Catalog items', report.items],
    ['People', report.people],
    ['Credits', report.credits],
    ['Segments', report.segments],
    ['Images', report.images],
    ['Artwork files', report.artwork_files],
    ['Watch history', report.watch_state],
    ['Recent searches', report.recent_searches],
    ['Activity entries', report.activity],
  ]);

  function plural(n: number, one: string, many: string): string {
    return `${n} ${n === 1 ? one : many}`;
  }
</script>

<div class="report">
  <p class="source">
    A bundle from Parlour {report.manifest.server_version}, exported
    {formatDateTime(report.manifest.exported_at)} (format v{report.manifest.format_version}).
    {#if report.manifest.hashes_included}
      <span class="chip bad">holds password and PIN hashes</span>
    {:else}
      <span class="chip">no password hashes</span>
    {/if}
    {#if report.manifest.artwork_included}<span class="chip">artwork included</span>{/if}
  </p>

  {#if adopted.length || locked.length || userConflicts.length || report.conflicts_total}
    <div class="heads-up" role="status">
      <strong>{will ? 'Before you apply' : 'What happened'}</strong>
      <ul>
        {#each adopted as u (u.username)}
          <li>
            This server’s first admin account, “{u.existing_username ?? u.username}”, was never
            used, so “{u.username}” from the bundle {will ? 'takes it over' : 'took it over'}:
            same account, so you stay signed in.
            {#if u.sign_in === 'from_bundle'}
              Afterwards it signs in with the old server’s password for “{u.username}”.
            {:else}
              The bundle has no hashes, so it keeps the password it has now.
            {/if}
          </li>
        {/each}
        {#if locked.length}
          <li class="warn">
            This bundle carries no password hashes:
            {plural(locked.length, 'account', 'accounts')}
            ({locked.map((u) => u.username).join(', ')})
            {will ? 'will be' : locked.length === 1 ? 'was' : 'were'} created with no usable
            password. Nobody can sign in as
            them until an admin sets one under
            <a class="link" href="/admin/users">Server → Users</a>. Profiles that had a PIN get one
            nobody knows.
          </li>
        {/if}
        {#if userConflicts.length}
          <li class="warn">
            {plural(userConflicts.length, 'account clashes', 'accounts clash')} with a different
            account here and {will ? 'will be' : userConflicts.length === 1 ? 'was' : 'were'} skipped,
            with their profiles, access and
            watch history.
          </li>
        {/if}
        {#if report.conflicts_total}
          <li>{plural(report.conflicts_total, 'conflict', 'conflicts')} — listed below.</li>
        {/if}
      </ul>
    </div>
  {/if}

  <h4>Accounts</h4>
  {#if report.users.length === 0}
    <p class="dim">The bundle has no accounts.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th>Username</th><th>What happens</th><th>Signs in</th><th></th></tr>
        </thead>
        <tbody>
          {#each report.users as u (u.username)}
            <tr>
              <td>
                {u.username}{#if u.is_admin}<span class="dim"> · admin</span>{/if}
                {#if u.existing_username && u.existing_username !== u.username}
                  <span class="dim"> (here: {u.existing_username})</span>
                {/if}
              </td>
              <td class:bad={u.action === 'conflict'}>{USER_ACTION[u.action] ?? u.action}</td>
              <td class:bad={u.sign_in === 'locked'}>{SIGN_IN[u.sign_in] ?? u.sign_in}</td>
              <td class="dim">{u.note ?? ''}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <h4>Libraries</h4>
  {#if report.libraries.length === 0}
    <p class="dim">The bundle has no libraries.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th>Library</th><th>What happens</th><th>Folders</th></tr>
        </thead>
        <tbody>
          {#each report.libraries as lib (lib.name)}
            <tr>
              <td>{lib.name}<span class="dim"> · {lib.kind}</span></td>
              <td class:bad={lib.action === 'conflict'}>
                {LIBRARY_ACTION[lib.action] ?? lib.action}
                {#if lib.note}<br /><span class="dim">{lib.note}</span>{/if}
              </td>
              <td>
                <div class="dim small">On the old server:</div>
                {#each lib.bundle_paths as p (p.path)}
                  <div class="mono">
                    {p.path}
                    <span class:ok={p.exists_here} class:bad={!p.exists_here}>
                      {p.exists_here ? '— exists here' : '— not on this server'}
                    </span>
                  </div>
                {/each}
                {#if lib.paths_added.length}
                  <div class="dim small">{will ? 'Will be added' : 'Added'}:</div>
                  {#each lib.paths_added as p (p.path)}
                    <div class="mono">
                      {p.path}
                      {#if !p.exists_here}<span class="bad">— not on this server</span>{/if}
                    </div>
                  {/each}
                {:else if lib.action !== 'conflict'}
                  <div class="dim small">No folders added.</div>
                {/if}
                {#if folders && lib.action !== 'conflict'}
                  <label class="small">
                    Folders on this server instead (one per line, optional)
                    <textarea
                      rows="2"
                      bind:value={folders[lib.name]}
                      placeholder="/media/{lib.name.toLowerCase()}"
                    ></textarea>
                  </label>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  <h4>Everything else</h4>
  <div class="table-wrap">
    <table class="counts">
      <thead>
        <tr><th></th><th>New</th><th>Updated</th><th>Already here</th><th>Skipped</th></tr>
      </thead>
      <tbody>
        {#each tallies as [label, t] (label)}
          <tr>
            <td>{label}</td>
            <td>{t.created}</td>
            <td>{t.updated}</td>
            <td>{t.existing}</td>
            <td class:bad={t.skipped > 0}>{t.skipped}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  {#if report.items_matched_by_provider_id}
    <p class="dim small">
      {plural(report.items_matched_by_provider_id, 'item', 'items')} matched on a provider id because
      its folder layout differs.
    </p>
  {/if}

  {#if report.conflicts.length}
    <h4>Conflicts</h4>
    <ul class="conflicts">
      {#each report.conflicts as c, i (i)}
        <li><span class="dim">{c.what}</span> <span class="mono">{c.key}</span> — {c.reason}</li>
      {/each}
    </ul>
    {#if report.conflicts_total > report.conflicts.length}
      <p class="dim small">
        …and {report.conflicts_total - report.conflicts.length} more not listed.
      </p>
    {/if}
  {/if}

  {#if report.notes.length}
    <h4>Notes</h4>
    <ul class="notes">
      {#each report.notes as note, i (i)}<li>{note}</li>{/each}
    </ul>
  {/if}
</div>

<style>
  .report {
    max-width: 60rem;
  }
  .source {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }
  h4 {
    margin: 1.2rem 0 0.4rem;
    font-size: 0.95rem;
  }
  .heads-up {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 0.7rem 0.9rem;
    margin: 0.6rem 0 1rem;
  }
  .heads-up ul {
    margin: 0.4rem 0 0;
    padding-left: 1.2rem;
    display: grid;
    gap: 0.35rem;
  }
  .warn {
    color: var(--accent);
  }
  .bad {
    color: var(--danger);
  }
  .ok {
    color: var(--ok);
  }
  .chip.bad {
    border-color: var(--danger);
  }
  .small {
    font-size: 0.85rem;
  }
  label.small {
    display: grid;
    gap: 0.25rem;
    margin-top: 0.5rem;
    color: var(--text-dim);
  }
  textarea {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.4rem 0.5rem;
    color: var(--text);
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
  }
  .counts td:not(:first-child),
  .counts th:not(:first-child) {
    text-align: right;
  }
  .conflicts,
  .notes {
    padding-left: 1.2rem;
    display: grid;
    gap: 0.3rem;
    overflow-wrap: anywhere;
  }
  .link {
    color: var(--accent);
  }
</style>
