<script lang="ts">
  import type { RiskTier, Safety } from "$lib/types";

  // Risk pill + explanatory hovercard, shared by the Health memory table and
  // both App List tables. The card answers two questions the pill can't:
  // *why* this classification (curated catalog, caution list, protected list,
  // or none of them = UNKNOWN) and *what the user can do about it* (mark the
  // app safe on this device / restore the classification). The action only
  // renders when the host passes `onToggleSafe` — read-only contexts (Optimize
  // wizard) get the explanation without the toggle.
  let {
    pkg,
    name,
    tier,
    description,
    safety = { kind: "safe" },
    overridden = false,
    busy = false,
    onToggleSafe,
  }: {
    pkg: string;
    name?: string;
    /// Curated-catalog risk tier; undefined when the package isn't curated.
    tier?: RiskTier;
    description?: string;
    /// Engine classification (never-disable / caution / safe).
    safety?: Safety;
    /// User marked this package safe on this device.
    overridden?: boolean;
    busy?: boolean;
    onToggleSafe?: (pkg: string) => void;
  } = $props();

  let blocked = $derived(safety.kind === "never_disable");
  let caution = $derived(safety.kind === "caution");
  let reason = $derived(safety.kind !== "safe" ? safety.reason : null);
  let label = $derived(
    blocked ? "SYSTEM" : overridden ? "SAFE" : caution ? "CAUTION" : tier ? tier.toUpperCase() : "UNKNOWN",
  );
  let cls = $derived(
    blocked ? "blocked" : overridden ? "safe" : caution ? "medium" : (tier ?? "unknown"),
  );
  /// Anything the user is allowed to wave off: caution, curated non-safe
  /// tiers, and unknowns. Catalog-safe needs no override; SYSTEM refuses one.
  let markable = $derived(!blocked && (caution || tier !== "safe"));
</script>

<span class="risk-wrap">
  <button type="button" class={`risk-pill pill-${cls}`} aria-label={`Risk details for ${pkg}`}>
    {label}
  </button>
  <span class="hovercard" role="tooltip">
    <span class="hc-name">{name ?? pkg}</span>
    <span class="hc-pkg">{pkg}</span>
    <span class="hc-risk">Risk: <strong class={`tone-${cls}`}>{label}</strong></span>
    {#if overridden}
      <p class="hc-body hc-ok">
        ✓ You marked this app safe on this device — risk warnings and loud
        confirms are suppressed. Shield Optimizer remembers this next time you
        connect this device.
      </p>
    {/if}
    {#if description}
      <p class="hc-body">{description}</p>
    {/if}
    {#if reason}
      <p class="hc-body hc-why">⚠ {reason}</p>
    {/if}
    <p class="hc-how">
      {#if blocked}
        How this was decided: this package is on the protected system list —
        disabling it can brick the device or cut off ADB, so it's hard-blocked.
      {:else if caution}
        How this was decided: this package is on the curated caution list —
        recoverable, but disabling visibly degrades the device.
      {:else if tier}
        How this was decided: this app is in the curated catalog with risk tier
        {tier.toUpperCase()}, based on what breaks when it's disabled.
      {:else}
        How this was decided: this package is not in the curated app catalog and
        not on the caution or protected system lists — nothing is known about
        the impact of disabling it, so it defaults to UNKNOWN. If you know this
        app, you can mark it safe below.
      {/if}
    </p>
    {#if blocked}
      <p class="hc-note">🔒 Protected system package — can't be disabled or marked safe (it would brick the device).</p>
    {:else if overridden}
      <button
        class="hc-action subtle"
        onclick={() => onToggleSafe?.(pkg)}
        disabled={busy || !onToggleSafe}
      >
        {busy ? "Saving…" : "Restore risk classification"}
      </button>
    {:else if markable && onToggleSafe}
      <button class="hc-action" onclick={() => onToggleSafe?.(pkg)} disabled={busy}>
        {busy ? "Saving…" : "Mark as safe on this device"}
      </button>
    {:else if !markable}
      <p class="hc-note">Already classified safe.</p>
    {/if}
  </span>
</span>

<style>
  .risk-wrap {
    position: relative;
    display: inline-block;
  }
  /* The pill is the hover/focus trigger. Color-coded surface + label — never
     color alone — matching the StateBadge pill geometry so the State and Risk
     columns read as one system. */
  .risk-pill {
    display: inline-block;
    font-family: ui-monospace, monospace;
    font-size: 0.74rem;
    padding: 0.15rem 0.55rem;
    border-radius: 4px;
    letter-spacing: 0.04em;
    border: none;
    margin: 0;
    line-height: 1.4;
    cursor: pointer;
  }
  .risk-pill:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .pill-safe { background: var(--ok-surface); color: var(--ok); }
  .pill-medium { background: var(--warn-surface-2); color: var(--warn); }
  .pill-high { background: var(--danger-surface); color: var(--danger-strong); }
  .pill-advanced {
    background: color-mix(in srgb, var(--advanced) 16%, transparent);
    color: var(--advanced);
  }
  .pill-unknown { background: var(--bg-muted); color: var(--fg-secondary); }
  .pill-blocked { background: var(--bg-muted); color: var(--fg-muted); }
  .tone-safe { color: var(--ok); }
  .tone-medium { color: var(--warn); }
  .tone-high { color: var(--danger-strong); }
  .tone-advanced { color: var(--advanced); }
  .tone-unknown { color: var(--fg-secondary); }
  .tone-blocked { color: var(--fg-muted); }

  /* The card is a *sibling* of the trigger button (not a child — a button
     can't contain the card's action button), both inside a position:relative
     wrapper. A transparent ::before bridge spans the 6px gap so moving the
     cursor from trigger to card never drops :hover. */
  .hovercard {
    position: absolute;
    z-index: 60;
    top: calc(100% + 6px);
    right: 0;
    width: 290px;
    max-width: 78vw;
    text-align: left;
    color: var(--fg-primary);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 0.7rem 0.8rem;
    box-shadow: 0 6px 22px rgba(0, 0, 0, 0.35);
    font-family: system-ui, -apple-system, sans-serif;
    font-size: 0.85rem;
    letter-spacing: normal;
    /* Host cells set nowrap to keep the pill on one line; the card's prose
       must wrap regardless of which cell it's mounted in. */
    white-space: normal;
    opacity: 0;
    visibility: hidden;
    transform: translateY(-3px);
    transition:
      opacity 0.1s ease,
      transform 0.1s ease,
      visibility 0.1s;
    pointer-events: none;
  }
  .hovercard::before {
    content: "";
    position: absolute;
    top: -8px;
    left: 0;
    right: 0;
    height: 8px;
  }
  .risk-wrap:hover .hovercard,
  .risk-wrap:focus-within .hovercard {
    opacity: 1;
    visibility: visible;
    transform: translateY(0);
    pointer-events: auto;
  }
  .hc-name {
    display: block;
    font-weight: 600;
    font-size: 0.9rem;
  }
  .hc-pkg {
    display: block;
    font-family: ui-monospace, monospace;
    font-size: 0.72rem;
    color: var(--fg-faint);
    margin-top: 0.1rem;
    word-break: break-all;
  }
  .hc-risk {
    display: block;
    font-size: 0.78rem;
    color: var(--fg-secondary);
    margin-top: 0.45rem;
  }
  .hc-body {
    margin: 0.45rem 0 0;
    font-size: 0.82rem;
    line-height: 1.35;
    color: var(--fg-primary);
  }
  .hc-why {
    color: var(--warn);
  }
  .hc-ok {
    color: var(--ok);
  }
  .hc-how {
    margin: 0.45rem 0 0;
    font-size: 0.78rem;
    line-height: 1.35;
    color: var(--fg-muted);
  }
  .hc-note {
    margin: 0.55rem 0 0;
    font-size: 0.78rem;
    color: var(--fg-muted);
  }
  .hc-action {
    margin-top: 0.6rem;
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
  }
  .hc-action.subtle {
    background: transparent;
    border-color: var(--border);
    color: var(--fg-muted);
  }
  .hc-action.subtle:hover:not(:disabled) {
    background: var(--bg-button);
    color: var(--fg-secondary);
  }
</style>
