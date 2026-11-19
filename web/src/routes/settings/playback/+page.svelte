<script lang="ts">
  // Preferences → Playback: audio language, the remote bitrate
  // cap, what the player does with intros, recaps and credits
  // (SEG5, D22 — playback-info hands the player this profile's
  // choice), and the autoplay-next choice (D15), which the player
  // reads once SEG6 lands.
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
    <legend>Intros, recaps and credits</legend>
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
  <p class="hint">
    Skip automatically skips each one the first time playback gets there, and says so. Go back
    into it and it plays, with the Skip button. Skipping credits that run to the very end finishes
    the episode, as if you had watched them.
  </p>

  <label class="check">
    <input type="checkbox" bind:checked={form.autoplay_next} />
    Play the next episode automatically, after a short countdown
  </label>
  <p class="hint">
    The player doesn't play the next episode by itself yet. This choice is saved now and applies
    once it does.
  </p>

  <p><button class="btn primary" type="submit" disabled={saving}>Save</button></p>
</form>
