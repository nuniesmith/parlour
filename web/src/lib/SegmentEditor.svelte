<script lang="ts">
  // Segment editor (SEG4, D21): the admin-only Segments section of
  // the item page. Lists every stored intro / credits / recap row
  // with what it means for playback now; adds and edits the admin's
  // own times (saved as source "manual", which always wins); and
  // deletes per D21 — the admin's own row is simply removed (detection
  // decides that kind again), while deleting a detected row stores a
  // "no segment of this kind here" marker that later detection runs
  // can't undo. "Preview" opens the player at a time in a mode that
  // reports nothing. The API is the real gate: every route here
  // answers 404 to non-admins.
  import { tick, untrack } from 'svelte';
  import { api } from '$lib/api';
  import {
    KIND_LABELS,
    SEGMENT_KINDS,
    SOURCE_LABELS,
    formatSegmentLength,
    formatSegmentTime,
    parseSegmentTime,
    previewHref,
    statusText,
  } from '$lib/segments';
  import type { SegmentEditorView, SegmentKind, StoredSegment } from '$lib/types';

  let { itemId, itemKind = null }: { itemId: string; itemKind?: string | null } = $props();

  let view = $state<SegmentEditorView | null>(null);
  let loadError = $state<string | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  /** The open times form: a new segment, or new times for one kind
   *  (opened from that kind's row, and shown under it). */
  let form = $state<{
    kind: SegmentKind;
    /** The row whose Edit opened it; null when adding. */
    anchor: string | null;
    start: string;
    end: string;
  } | null>(null);
  let startInput = $state<HTMLInputElement | null>(null);

  const rows = $derived(view?.rows ?? []);
  const fileMs = $derived(view?.file?.duration_ms ?? null);
  /** Kinds that already have the admin's times: change those with Edit. */
  const kindsWithTimes = $derived(
    new Set(rows.filter((r) => r.source === 'manual' && r.start_ms !== null).map((r) => r.kind)),
  );
  const addableKinds = $derived(SEGMENT_KINDS.filter((k) => !kindsWithTimes.has(k)));
  /** Kinds marked "none" by an admin (D21). */
  const markedKinds = $derived(
    new Set(rows.filter((r) => r.source === 'manual' && r.start_ms === null).map((r) => r.kind)),
  );
  /** What saving the open form will replace, when that isn't obvious. */
  const formNote = $derived.by(() => {
    if (!form) return null;
    const kind = kindWord(form.kind);
    const fromDetected = form.anchor !== null && !form.anchor.endsWith('-manual');
    if (markedKinds.has(form.kind)) return `Saving replaces the “no ${kind}” mark with these times.`;
    if (fromDetected && kindsWithTimes.has(form.kind)) {
      return `Saving replaces the ${kind} times set by hand before.`;
    }
    if (fromDetected) {
      return `Saving stores these as this item’s ${kind} times, used instead of what detection found.`;
    }
    return null;
  });

  const parsed = $derived(
    form
      ? { start: parseSegmentTime(form.start), end: parseSegmentTime(form.end) }
      : { start: null, end: null },
  );
  /** What is wrong with the form's times, once both are filled in. The
   *  server checks the same (D21: start before end, both in the file). */
  const problem = $derived.by(() => {
    if (!form || form.start.trim() === '' || form.end.trim() === '') return null;
    const hint = 'use h:mm:ss, m:ss or seconds, e.g. 1:30.5';
    if (parsed.start === null) return `The start isn’t a time — ${hint}.`;
    if (parsed.end === null) return `The end isn’t a time — ${hint}.`;
    if (parsed.end <= parsed.start) return 'The start must be before the end.';
    if (fileMs !== null && parsed.end > fileMs) {
      return `The end is past the end of the file (${formatSegmentTime(fileMs)}).`;
    }
    return null;
  });
  const canSave = $derived(
    !!form && !busy && parsed.start !== null && parsed.end !== null && problem === null,
  );

  function key(row: StoredSegment): string {
    return `${row.kind}-${row.source}`;
  }
  function kindWord(kind: SegmentKind): string {
    return KIND_LABELS[kind].toLowerCase();
  }
  /** "Manual", or a detector with its confidence: "Chapters · 100%". */
  function sourceLabel(row: StoredSegment): string {
    const name = SOURCE_LABELS[row.source];
    return row.source === 'manual' ? name : `${name} · ${Math.round(row.confidence * 100)}%`;
  }
  function isMarker(row: StoredSegment): boolean {
    return row.start_ms === null || row.end_ms === null;
  }
  /** Detected rows can be deleted while detection decides their kind;
   *  once the admin has times or a marker for it, those come first. */
  function deletable(row: StoredSegment): boolean {
    if (row.source === 'manual') return !isMarker(row);
    return row.status !== 'overridden' && row.status !== 'hidden';
  }
  /** A manual row that no longer fits the file (it was replaced by a
   *  shorter one since): still served, but worth a look. */
  function pastFileEnd(row: StoredSegment): boolean {
    return row.source === 'manual' && row.end_ms !== null && fileMs !== null && row.end_ms > fileMs;
  }

  async function fetchView(id: string) {
    return api<SegmentEditorView>(`/api/v1/admin/items/${id}/segments`);
  }

  async function load(id: string) {
    view = null;
    loadError = null;
    error = null;
    notice = null;
    form = null;
    const { data, error: err } = await fetchView(id);
    if (id !== itemId) return; // a newer item replaced this one
    if (data) {
      view = data;
      // Arriving from the player's "Back to segments" link.
      if (location.hash === '#segments') {
        await tick();
        document.getElementById('segments')?.scrollIntoView();
      }
    } else loadError = err ?? 'Could not load the segments.';
  }

  $effect(() => {
    const id = itemId;
    untrack(() => void load(id));
  });

  async function focusStart() {
    await tick();
    startInput?.focus();
  }

  function openAdd() {
    error = null;
    notice = null;
    form = { kind: addableKinds[0] ?? 'intro', anchor: null, start: '', end: '' };
    void focusStart();
  }

  function openEdit(row: StoredSegment) {
    error = null;
    notice = null;
    form = {
      kind: row.kind,
      anchor: key(row),
      start: row.start_ms === null ? '' : formatSegmentTime(row.start_ms),
      end: row.end_ms === null ? '' : formatSegmentTime(row.end_ms),
    };
    void focusStart();
  }

  async function save(event: SubmitEvent) {
    event.preventDefault();
    if (!form || !canSave || parsed.start === null || parsed.end === null) return;
    const kind = form.kind;
    busy = true;
    error = null;
    notice = null;
    const { data, error: err } = await api<SegmentEditorView>(
      `/api/v1/admin/items/${itemId}/segments/${kind}`,
      { method: 'PUT', body: JSON.stringify({ start_ms: parsed.start, end_ms: parsed.end }) },
    );
    busy = false;
    if (data) {
      view = data;
      form = null;
      notice = `Saved the ${kindWord(kind)} times. They win over detection.`;
    } else error = err ?? 'Could not save those times.';
  }

  async function remove(row: StoredSegment) {
    const kind = kindWord(row.kind);
    if (row.source === 'manual' && !isMarker(row)) {
      if (!confirm(`Delete your ${kind} times? Detection decides this item’s ${kind} again.`)) {
        return;
      }
    } else if (row.source !== 'manual') {
      const ok = confirm(
        `Delete the detected ${kind}? The player gets no ${kind} for this item, and later ` +
          `detection runs won’t bring one back — until you choose “Let detection decide”.`,
      );
      if (!ok) return;
    }
    busy = true;
    error = null;
    notice = null;
    const { data, error: err } = await api<SegmentEditorView>(
      `/api/v1/admin/items/${itemId}/segments/${row.kind}/${row.source}`,
      { method: 'DELETE' },
    );
    busy = false;
    if (data) {
      view = data;
      form = null;
      notice =
        row.source !== 'manual'
          ? `This item now has no ${kind}. Detection won’t add one back.`
          : isMarker(row)
            ? `Detection decides the ${kind} again.`
            : `Deleted your ${kind} times. Detection decides the ${kind} again.`;
    } else {
      error = err ?? 'Could not delete that segment.';
      // Someone else may have changed it: show what is stored now.
      const fresh = await fetchView(itemId);
      if (fresh.data) view = fresh.data;
    }
  }

  function onFormKey(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      form = null;
    }
  }
</script>

{#snippet timesForm()}
  {#if form}
    <form class="times-form" onsubmit={save}>
      <p class="form-title">
        {#if form.anchor}
          {KIND_LABELS[form.kind]} times
        {:else}
          Add a segment
        {/if}
      </p>
      <div class="form-row">
        {#if !form.anchor}
          <label>
            <span>Kind</span>
            <select bind:value={form.kind} onkeydown={onFormKey}>
              {#each addableKinds as k (k)}
                <option value={k}>{KIND_LABELS[k]}</option>
              {/each}
            </select>
          </label>
        {/if}
        <label>
          <span>Start</span>
          <input
            bind:this={startInput}
            bind:value={form.start}
            onkeydown={onFormKey}
            placeholder="0:45"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <label>
          <span>End</span>
          <input
            bind:value={form.end}
            onkeydown={onFormKey}
            placeholder="1:30"
            autocomplete="off"
            spellcheck="false"
          />
        </label>
      </div>
      <p class="previews">
        {#if parsed.start !== null}
          <a
            class="btn small"
            href={previewHref(itemId, parsed.start)}
            target="_blank"
            rel="noopener"
          >▶ Preview from {formatSegmentTime(parsed.start)}</a>
        {/if}
        {#if parsed.end !== null}
          <a class="btn small" href={previewHref(itemId, parsed.end)} target="_blank" rel="noopener"
            >▶ Preview from {formatSegmentTime(parsed.end)}</a
          >
        {/if}
        <span class="dim hint">
          Times as h:mm:ss, m:ss or seconds (1:30.5). Previews open in a new tab, so unsaved
          times stay here.
        </span>
      </p>
      {#if problem}<p class="error" role="alert">{problem}</p>{/if}
      {#if formNote}<p class="dim">{formNote}</p>{/if}
      <p class="form-actions">
        <button class="btn primary small" type="submit" disabled={!canSave}>Save</button>
        <button class="btn small" type="button" onclick={() => (form = null)}>Cancel</button>
      </p>
    </form>
  {/if}
{/snippet}

<section class="segments" id="segments" aria-labelledby="segments-title">
  <h2 id="segments-title">Segments</h2>
  {#if loadError}
    <p class="error" role="alert">{loadError}</p>
  {:else if !view}
    <p class="dim">Loading segments…</p>
  {:else if !view.file}
    <p class="dim">This item has no playable file, so it has no segments.</p>
  {:else}
    <p class="dim about">
      Only admins see this. Times are into
      <span class="mono">{view.file.relative_path}</span>{fileMs !== null
        ? ` (${formatSegmentTime(fileMs)} long)`
        : ''}. Times set here always win over detection; deleting a detected segment marks the
      item as having none.
    </p>
    {#if notice}<p class="notice" role="status">{notice}</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}

    {#if rows.length === 0}
      <p class="dim">
        {#if itemKind === 'movie'}
          Nothing stored. Detection only looks at series episodes, so a movie’s segments are added
          here by hand.
        {:else}
          Nothing stored: detection hasn’t found a segment in this file, and none has been added.
        {/if}
      </p>
    {:else}
      <ul class="rows">
        {#each rows as row (key(row))}
          <li class="row" class:used={row.status === 'used' || row.status === 'no_segment'}>
            <div class="what">
              <strong class="kind">{KIND_LABELS[row.kind]}</strong>
              {#if row.start_ms !== null && row.end_ms !== null}
                <span class="times">
                  {formatSegmentTime(row.start_ms)} – {formatSegmentTime(row.end_ms)}
                </span>
                <span class="dim">({formatSegmentLength(row.end_ms - row.start_ms)})</span>
              {:else}
                <span class="times">None</span>
              {/if}
              <span class="chip">{sourceLabel(row)}</span>
            </div>
            <p class="status" class:dim={row.status !== 'used'}>{statusText(row)}</p>
            {#if pastFileEnd(row)}
              <p class="error small-print">
                Ends after the file does — the file may have changed since these times were set.
              </p>
            {/if}
            <div class="row-actions">
              {#if row.start_ms !== null && row.end_ms !== null}
                <a
                  class="btn small"
                  href={previewHref(itemId, row.start_ms)}
                  aria-label={`Preview from the ${kindWord(row.kind)} start, ${formatSegmentTime(row.start_ms)}`}
                  >▶ From start</a
                >
                <a
                  class="btn small"
                  href={previewHref(itemId, row.end_ms)}
                  aria-label={`Preview from the ${kindWord(row.kind)} end, ${formatSegmentTime(row.end_ms)}`}
                  >▶ From end</a
                >
                <button class="btn small" disabled={busy} onclick={() => openEdit(row)}>Edit</button>
              {/if}
              {#if row.status === 'no_segment'}
                <button class="btn small" disabled={busy} onclick={() => remove(row)}>
                  Let detection decide
                </button>
              {:else if deletable(row)}
                <button class="btn small danger" disabled={busy} onclick={() => remove(row)}>
                  Delete
                </button>
              {/if}
            </div>
            {#if form && form.anchor === key(row)}
              {@render timesForm()}
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if form && form.anchor === null}
      {@render timesForm()}
    {:else if addableKinds.length > 0}
      <p><button class="btn small" disabled={busy} onclick={openAdd}>+ Add a segment</button></p>
    {/if}
  {/if}
</section>

<style>
  .segments {
    margin-top: 1.6rem;
  }
  .about {
    max-width: 75ch;
  }
  .mono {
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
    word-break: break-all;
  }
  .rows {
    list-style: none;
    margin: 0 0 0.8rem;
    padding: 0;
    display: grid;
    gap: 0.6rem;
  }
  .row {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.6rem;
    padding: 0.7rem 1rem;
  }
  .row.used {
    border-color: var(--accent);
  }
  .what {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .kind {
    min-width: 4.5rem;
  }
  .times {
    font-variant-numeric: tabular-nums;
  }
  .status {
    margin: 0.3rem 0 0;
    font-size: 0.9rem;
  }
  .small-print {
    margin: 0.3rem 0 0;
    font-size: 0.85rem;
  }
  .row-actions {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    margin-top: 0.5rem;
  }
  .times-form {
    margin-top: 0.7rem;
    padding: 0.7rem 1rem;
    border: 1px dashed var(--line);
    border-radius: 0.6rem;
    background: var(--bg-card);
  }
  .form-title {
    margin: 0 0 0.4rem;
    font-weight: 600;
  }
  .form-row {
    display: flex;
    gap: 0.7rem;
    flex-wrap: wrap;
  }
  .form-row label {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
    color: var(--text-dim);
  }
  .form-row input,
  .form-row select {
    background: var(--bg-raise);
    border: 1px solid var(--line);
    border-radius: 0.5rem;
    padding: 0.45rem 0.6rem;
    color: var(--text);
    width: 9rem;
    font-variant-numeric: tabular-nums;
  }
  .previews {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    align-items: center;
    margin: 0.6rem 0 0;
  }
  .hint {
    font-size: 0.85rem;
  }
  .form-actions {
    display: flex;
    gap: 0.4rem;
    margin: 0.6rem 0 0;
  }
  button:disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>
