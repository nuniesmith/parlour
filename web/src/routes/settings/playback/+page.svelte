<script lang="ts">
  // Preferences → Playback: audio language, the remote bitrate
  // cap, and the intro/credits skip and autoplay-next choices
  // (D15) that the player reads once Phase 9 lands.
  import { session } from '$lib/session.svelte';
  import { updateActiveProfile } from '$lib/preferences';

  const p = session.me?.active_profile;
  let form = $state({
    audio_lang: p?.audio_lang ?? '',
    remote_cap: p?.remote_bitrate_cap_kbps ? String(p.remote_bitrate_cap_kbps) : '',
    segment_skip: p?.segment_skip ?? 'button',
    autoplay_next: p?.autoplay_next ?? true,
  });
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);

  async function save(event: SubmitEvent) {
    event.preventDefault();
    error = null;
    notice = null;
    const body: Record<string, unknown> = {
      segment_skip: form.segment_skip,
      autoplay_next: form.autoplay_next,
    };
    const lang = form.audio_lang.trim();
    if (lang) body.audio_lang = lang;
    else body.clear_audio_lang = true;
    const cap = form.remote_cap.trim();
    if (cap) {
      const kbps = Number(cap);
      if (!Number.isInteger(kbps) || kbps <= 0) {
        error = 'Bitrate cap must be a whole number of kbps, or blank for no cap.';
        return;
      }
      body.remote_bitrate_cap_kbps = kbps;
    } else {
      body.clear_bitrate_cap = true;
    }
    saving = true;
    const r = await updateActiveProfile(body);
    saving = false;
    if (r.ok) notice = 'Saved.';
    else error = r.error;
  }
</script>

<h2>Playback</h2>

<form class="form" onsubmit={save}>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if notice}<p class="notice" role="status">{notice}</p>{/if}
  <div class="field">
    <label for="audiolang">Preferred audio language (e.g. en; blank = the file's default)</label>
    <input id="audiolang" bind:value={form.audio_lang} maxlength="8" />
  </div>
  <div class="field">
    <label for="cap">Remote bitrate cap, kbps (blank = no cap)</label>
    <input id="cap" bind:value={form.remote_cap} inputmode="numeric" />
  </div>

  <fieldset>
    <legend>Intros and credits</legend>
    <label class="check">
      <input type="radio" name="skip" value="button" bind:group={form.segment_skip} />
      Show a Skip button
    </label>
    <label class="check">
      <input type="radio" name="skip" value="auto" bind:group={form.segment_skip} />
      Skip automatically
    </label>
    <label class="check">
      <input type="radio" name="skip" value="off" bind:group={form.segment_skip} />
      Don't offer to skip
    </label>
  </fieldset>

  <label class="check">
    <input type="checkbox" bind:checked={form.autoplay_next} />
    Play the next episode automatically, after a short countdown
  </label>
  <p class="hint">
    The player doesn't detect intros and credits or play the next episode yet. These choices are
    saved now and apply once it does.
  </p>

  <p><button class="btn primary" type="submit" disabled={saving}>Save</button></p>
</form>
