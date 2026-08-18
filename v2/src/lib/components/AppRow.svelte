<script lang="ts">
  import type { Snippet } from "svelte";
  import type { AppUsage, RiskTier, Safety } from "$lib/types";
  import StateBadge from "$lib/components/StateBadge.svelte";
  import RamBadge from "$lib/components/RamBadge.svelte";
  import UsageBadge from "$lib/components/UsageBadge.svelte";
  import RiskBadge from "$lib/components/RiskBadge.svelte";

  // One catalog-app table row, shared by the App List and the Optimize wizard.
  // Dumb on purpose: data in, an `actions` snippet for the per-tab buttons —
  // the row owns layout (name/desc/pkg, state+RAM+usage cluster, risk), never
  // behaviour. Whether a cue shows (`mb`, `usage`) is the caller's call; the
  // badges already self-hide when their value is falsy. The risk pill's
  // mark-safe action only appears when the host passes `onToggleSafe`.
  let {
    name,
    description,
    package: pkg,
    review = false,
    state,
    mb,
    ramLabel = true,
    usage,
    showUsage = true,
    risk,
    kind,
    usageColumn = false,
    usageWindowSecs = null,
    safety,
    overridden = false,
    riskBusy = false,
    onToggleSafe,
    rowClass,
    actions,
  }: {
    name: string;
    description?: string;
    package: string;
    review?: boolean;
    state: "enabled" | "disabled" | "missing";
    mb?: number;
    ramLabel?: boolean;
    usage?: AppUsage;
    showUsage?: boolean;
    /// Render the last-used cue as its own cell after State instead of stacking
    /// it under the state badge. Opt-in: the host must add the matching
    /// `<th>Last used</th>`.
    usageColumn?: boolean;
    /// Device uptime — bounds how far back the usage data can see.
    usageWindowSecs?: number | null;
    /// Curated risk tier; omitted for packages outside the catalog, which the
    /// risk pill then renders as UNKNOWN off the safety map alone.
    risk?: RiskTier;
    /// Provenance chip next to the name. Only the non-curated kinds get one —
    /// a curated row already announces itself with a description + a
    /// recommendation.
    kind?: "curated" | "user" | "system";
    safety?: Safety;
    overridden?: boolean;
    riskBusy?: boolean;
    onToggleSafe?: (pkg: string) => void;
    rowClass?: string;
    actions: Snippet;
  } = $props();
</script>

<tr class={rowClass}>
  <td class="app-cell">
    <div class="app-name-row">
      {name}
      {#if kind === "user"}
        <span class="tag kind-user" title="Installed by you — not part of the stock system image">USER</span>
      {:else if kind === "system"}
        <span class="tag kind-system" title="Preinstalled with the system image">SYSTEM</span>
      {/if}
      {#if review}
        <span class="tag review" title="Remove if you don't use it">REVIEW</span>
      {/if}
    </div>
    {#if description}
      <div class="muted small app-desc">{description}</div>
    {/if}
    <div class="muted small mono pkg-id">{pkg}</div>
  </td>
  <td class="center cluster-cell">
    <StateBadge {state} />
    {#if mb}
      <div class="cell-cue"><RamBadge {mb} label={ramLabel} /></div>
    {/if}
    {#if usage && showUsage && !usageColumn}
      <div class="cell-cue"><UsageBadge {usage} windowSecs={usageWindowSecs} /></div>
    {/if}
  </td>
  {#if usageColumn}
    <td class="center usage-col">
      {#if usage && showUsage}
        <UsageBadge {usage} compact windowSecs={usageWindowSecs} />
      {:else}
        <span class="usage-none">—</span>
      {/if}
    </td>
  {/if}
  <td class="center risk-cell">
    <RiskBadge
      {pkg}
      {name}
      tier={risk}
      {description}
      {safety}
      {overridden}
      busy={riskBusy}
      {onToggleSafe}
    />
  </td>
  {@render actions()}
</tr>

<style>
  /* The risk pill + its colors live in RiskBadge. The table chrome (th/td
     borders, padding, .center) is owned by the host table; this row only
     styles the cells it fully owns. */
  td {
    text-align: left;
    padding: 0.5rem 0.6rem;
    border-bottom: 1px solid var(--bg-button);
    vertical-align: middle;
  }
  td.center {
    text-align: center;
  }
  .app-cell {
    line-height: 1.3;
    /* Long system package ids are one unbreakable token; without this they
       force the column — and the whole table — wider than the viewport.
       `anywhere` also shrinks the column's min-content width. Inherited by the
       child name/pkg rows. */
    overflow-wrap: anywhere;
  }
  .app-name-row {
    font-size: 0.95rem;
    font-weight: 500;
  }
  .app-desc {
    margin-top: 0.15rem;
    font-size: 0.82rem;
    max-width: 42rem;
  }
  .pkg-id {
    margin-top: 0.1rem;
    font-size: 0.78rem;
    opacity: 0.7;
  }
  /* Small stacked cue (RAM / last-used badge) under the row's state badge. */
  .cell-cue {
    margin-top: 0.2rem;
  }
  .usage-col {
    white-space: nowrap;
    font-size: 0.78rem;
  }
  .usage-none {
    color: var(--fg-faint);
  }
  .risk-cell {
    overflow: visible;
    white-space: nowrap;
  }
  .tag {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border-radius: 4px;
    letter-spacing: 0.04em;
  }
  .tag.review {
    background: var(--warn-surface-2);
    color: var(--warn);
  }
  /* Provenance chips read as metadata, not as a warning — muted next to the
     name so REVIEW keeps its emphasis. */
  .tag.kind-user,
  .tag.kind-system {
    background: var(--bg-inset);
    color: var(--fg-muted);
    border: 1px solid var(--border);
  }
  .small {
    font-size: 0.82rem;
  }
  .mono {
    font-family: ui-monospace, monospace;
  }
  /* Optimize-row emphasis (passed via rowClass): skipped rows recede; rows that
     WILL be acted on get a left accent bar and a faint tint. The action/result
     cells come in through the actions snippet, so the tint reaches them via
     :global. */
  tr.dim {
    opacity: 0.78;
  }
  tr.acting :global(td) {
    background: color-mix(in srgb, var(--accent-strong) 8%, transparent);
  }
  tr.acting td:first-child {
    box-shadow: inset 3px 0 0 var(--accent-strong);
  }
  /* Review rows awaiting a human call: full opacity (unlike other skipped
     rows) + a warn accent bar — the wizard wants eyes here. */
  tr.review-flag td:first-child {
    box-shadow: inset 3px 0 0 var(--warn);
  }
</style>
