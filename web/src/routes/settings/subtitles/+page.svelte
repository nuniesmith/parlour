<script lang="ts">
  // Preferences → Subtitles: when subtitles turn on, and which
  // language to prefer. Playback-info reads both.
  import { session } from '$lib/session.svelte';
  import { updateActiveProfile } from '$lib/preferences';

  const p = session.me?.active_profile;
  let form = $state({
    subtitle_mode: p?.subtitle_mode ?? 'auto',
    subtitle_lang: p?.subtitle_lang ?? '',
  });
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    error = null;
    notice = null;
    const body: Record<string, unknown> = { subtitle_mode: form.subtitle_mode };
    const lang = form.subtitle_lang.trim();
    if (lang) body.subtitle_lang = lang;
    else body.clear_subtitle_lang = true;
    saving = true;
    const r = await updateActiveProfile(body);
    saving = false;
    if (r.ok) notice = 'Saved.';
    else error = r.error;
  }
</script>

<h2>Subtitles</h2>

<form class="form" onsubmit={save}>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  <div class="field">
    <label for="submode">Subtitles</label>
    <select id="submode" bind:value={form.subtitle_mode}>
      <option value="auto">Auto (forced / language match)</option>
      <option value="always">Always on</option>
      <option value="never">Never</option>
    </select>
  </div>
  <div class="field">
    <label for="sublang">Preferred subtitle language (e.g. en; blank = no preference)</label>
    <input id="sublang" bind:value={form.subtitle_lang} maxlength="8" />
  </div>
  <p><button class="btn primary" type="submit" disabled={saving}>Save</button></p>
</form>
