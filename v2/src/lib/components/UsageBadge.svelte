<script lang="ts">
  import type { AppUsage } from "$lib/types";
  import { isStaleUsage, usageLabel, usageShortLabel, windowNote } from "$lib/usage";

  // "last used" cue for the Review / remove-if-unused decision. Renders nothing
  // until usage data is loaded. Long-idle apps read as stale (highlighted) —
  // the candidates to remove. Interpretation lives in $lib/usage so the
  // Optimize review callout can't drift from this badge.
  //
  // `compact` drops the "last used" prefix, for hosts that put the badge in a
  // column already headed "Last used". `windowSecs` is device uptime, which
  // bounds how much history exists at all — without it an app with no record
  // would be called unused when the device simply rebooted an hour ago.
  let {
    usage,
    compact = false,
    windowSecs = null,
  }: { usage?: AppUsage; compact?: boolean; windowSecs?: number | null } = $props();
</script>

{#if usage}
  <span
    class="usage-tag"
    class:stale={isStaleUsage(usage, windowSecs)}
    title="Last foreground use from usagestats. {windowNote(windowSecs)}"
  >
    {compact ? usageShortLabel(usage, windowSecs) : usageLabel(usage, windowSecs)}
  </span>
{/if}

<style>
  .usage-tag {
    font-size: 0.72rem;
    color: var(--fg-muted);
  }
  .usage-tag.stale {
    color: var(--warn);
  }
</style>
