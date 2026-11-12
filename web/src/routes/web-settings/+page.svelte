<script lang="ts">
  // Web Settings → Keyboard: a reference for the shortcuts this
  // web app already answers to. Nothing here is stored server-side.
  const player: { keys: string[]; action: string }[] = [
    { keys: ['Space', 'K'], action: 'Play or pause' },
    { keys: ['→'], action: 'Forward 10 seconds' },
    { keys: ['←'], action: 'Back 10 seconds' },
    { keys: ['Shift + →'], action: 'Forward 30 seconds' },
    { keys: ['Shift + ←'], action: 'Back 30 seconds' },
    { keys: ['M'], action: 'Mute or unmute' },
    { keys: ['F'], action: 'Enter or leave full screen' },
  ];
  const everywhere: { keys: string[]; action: string }[] = [
    { keys: ['Tab', 'Shift + Tab'], action: 'Move between links, buttons and fields' },
    { keys: ['Enter'], action: 'Open the highlighted link or press the highlighted button' },
    { keys: ['Esc'], action: 'Close the menu drawer on small screens' },
  ];
</script>

<h2>Keyboard</h2>
<p class="dim">Shortcuts in this browser. They apply to the web app only, not to other devices.</p>

{#snippet table(caption: string, rows: { keys: string[]; action: string }[])}
  <table>
    <caption>{caption}</caption>
    <tbody>
      {#each rows as row (row.action)}
        <tr>
          <th scope="row">
            {#each row.keys as key, i (key)}{#if i > 0}<span class="dim"> or </span>{/if}<kbd
                >{key}</kbd
              >{/each}
          </th>
          <td>{row.action}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/snippet}

{@render table('While watching', player)}
{@render table('Everywhere', everywhere)}

<style>
  table {
    width: 100%;
    max-width: 36rem;
    border-collapse: collapse;
    margin-bottom: 1.6rem;
  }
  caption {
    text-align: left;
    color: var(--text-dim);
    font-size: 0.85rem;
    padding-bottom: 0.4rem;
  }
  th,
  td {
    text-align: left;
    font-weight: 400;
    padding: 0.55rem 0.6rem;
    border-top: 1px solid var(--line);
    vertical-align: top;
  }
  th {
    width: 45%;
    white-space: nowrap;
  }
  kbd {
    font: inherit;
    font-size: 0.85rem;
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.35rem;
    padding: 0.1rem 0.45rem;
  }
</style>
