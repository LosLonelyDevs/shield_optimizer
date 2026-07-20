<script lang="ts">
  import { api } from "$lib/api";
  import type { ActivityEntry } from "$lib/types";

  let { serial, active = false }: { serial: string; active?: boolean } = $props();

  let command = $state("");
  let output = $state("");
  let ok = $state(true);
  let busy = $state(false);

  // Background activity: incremental tail of every command the app runs
  // behind the scenes. Polled only while this tab is the active one — the
  // page keeps visited tabs mounted (hidden), so `active` gates the timer.
  let entries = $state<ActivityEntry[]>([]);
  let lastId = 0;
  let activityBox = $state<HTMLElement | null>(null);

  async function refreshActivity() {
    try {
      const fresh = await api.activityTail(lastId);
      if (fresh.length > 0) {
        lastId = fresh[fresh.length - 1].id;
        entries = [...entries, ...fresh].slice(-500);
      }
    } catch {
      // Backend unavailable mid-poll — retry on the next tick.
    }
  }

  function clearActivity() {
    // Client-side only: lastId stays, so cleared entries don't reappear.
    entries = [];
  }

  $effect(() => {
    if (!active) return;
    refreshActivity();
    const timer = setInterval(refreshActivity, 1500);
    return () => clearInterval(timer);
  });

  // Keep the log pinned to the newest entry as it grows.
  $effect(() => {
    entries;
    if (activityBox) activityBox.scrollTop = activityBox.scrollHeight;
  });

  function fmtTime(tsMs: number): string {
    return new Date(tsMs).toLocaleTimeString();
  }

  const BOOKMARK_KEY = "shieldopt.consoleBookmarks";
  // A few safe, useful starting points so the console isn't a blank box.
  const SEEDED = [
    "getprop ro.build.version.release",
    "dumpsys power | grep -i mWakefulness",
    "wm size",
    "pm list packages -3",
    "settings list global",
  ];

  function loadBookmarks(): string[] {
    try {
      const raw = localStorage.getItem(BOOKMARK_KEY);
      if (raw) return JSON.parse(raw) as string[];
    } catch {
      // Corrupt/blocked storage — fall back to the seed.
    }
    return SEEDED;
  }

  let bookmarks = $state<string[]>(loadBookmarks());

  function persist() {
    try {
      localStorage.setItem(BOOKMARK_KEY, JSON.stringify(bookmarks));
    } catch {
      // Best-effort; nothing to do if storage is unavailable.
    }
  }

  function addBookmark() {
    const c = command.trim();
    if (!c || bookmarks.includes(c)) return;
    bookmarks = [c, ...bookmarks].slice(0, 50);
    persist();
  }

  function removeBookmark(c: string) {
    bookmarks = bookmarks.filter((b) => b !== c);
    persist();
  }

  async function run() {
    const c = command.trim();
    if (!c || busy) return;
    busy = true;
    try {
      const r = await api.runShell(serial, c);
      ok = r.ok;
      output = r.output || "(no output)";
    } catch (e) {
      ok = false;
      output = String(e);
    } finally {
      busy = false;
    }
  }

  function useBookmark(c: string) {
    command = c;
    run();
  }
</script>

<div class="card" role="tabpanel" tabindex={0} id="tabpanel-console" aria-labelledby="tab-console">
  <div class="card-header">
    <h2>ADB Console</h2>
  </div>
  <div class="warn-banner">
    ⚠ <strong>Power user:</strong> this runs whatever you type as
    <code>adb shell &lt;command&gt;</code> on the device, with no safety catalog and no
    confirmation. There's no undo — know what a command does before you run it.
  </div>

  <div class="console-input">
    <input
      type="text"
      placeholder="pm list packages -3"
      bind:value={command}
      disabled={busy}
      onkeydown={(e) => e.key === "Enter" && run()}
      spellcheck="false"
      autocapitalize="off"
      autocorrect="off"
    />
    <button class="primary" disabled={busy || !command.trim()} onclick={run}>
      {busy ? "Running…" : "Run"}
    </button>
    <button class="small-action subtle" disabled={!command.trim()} onclick={addBookmark} title="Save this command to bookmarks">★ Save</button>
  </div>

  {#if output}
    <pre class="console-output" class:bad={!ok}>{output}</pre>
  {/if}

  <h3>Bookmarks</h3>
  {#if bookmarks.length === 0}
    <p class="muted small">No bookmarks. Type a command and hit ★ Save.</p>
  {:else}
    <ul class="bookmarks">
      {#each bookmarks as b (b)}
        <li>
          <button class="bm-run mono" onclick={() => useBookmark(b)} title="Run this command">{b}</button>
          <button class="bm-del" onclick={() => removeBookmark(b)} title="Remove bookmark" aria-label="Remove bookmark">✕</button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<div class="card activity-card">
  <div class="card-header">
    <h2>Background activity</h2>
    <button class="small-action subtle" disabled={entries.length === 0} onclick={clearActivity}>
      Clear
    </button>
  </div>
  <p class="muted small activity-hint">
    Every command the app runs behind the scenes — adb calls, scrcpy launches —
    with its output. If something fails silently (like a mirror window that
    never appears), the reason lands here.
  </p>
  {#if entries.length === 0}
    <p class="muted small">Nothing yet. Actions you take will log their commands here.</p>
  {:else}
    <ul class="activity" bind:this={activityBox}>
      {#each entries as e (e.id)}
        <li class:bad={!e.ok}>
          <div class="act-head">
            <span class="act-time mono">{fmtTime(e.ts_ms)}</span>
            <span class="act-src" class:bad={!e.ok}>{e.source}</span>
            <span class="act-cmd mono">{e.command}</span>
          </div>
          {#if e.output}
            <pre class="act-out">{e.output}</pre>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .card {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.2rem;
  }
  .card h2 {
    margin: 0 0 0.8rem;
    font-size: 1.1rem;
  }
  .card h3 {
    margin: 1rem 0 0.4rem;
    font-size: 1rem;
    color: var(--fg-secondary);
  }
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .small {
    font-size: 0.82rem;
  }
  .mono {
    font-family: ui-monospace, monospace;
  }
  .warn-banner {
    background: var(--danger-surface);
    color: var(--danger-text);
    border: 1px solid var(--danger-text);
    border-radius: 6px;
    padding: 0.6rem 0.8rem;
    font-size: 0.85rem;
    line-height: 1.45;
    margin-bottom: 0.8rem;
  }
  .console-input {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .console-input input {
    flex: 1;
    min-width: 240px;
    font-family: ui-monospace, monospace;
  }
  .console-output {
    margin: 0.7rem 0 0;
    padding: 0.7rem 0.9rem;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: ui-monospace, monospace;
    font-size: 0.82rem;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 360px;
    overflow: auto;
  }
  .console-output.bad {
    border-color: var(--danger-text);
    color: var(--danger-text);
  }
  .bookmarks {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }
  .bookmarks li {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }
  .bm-run {
    flex: 1;
    text-align: left;
    padding: 0.3rem 0.6rem;
    font-size: 0.8rem;
    background: var(--bg-button);
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bm-run:hover {
    background: var(--border);
  }
  .bm-del {
    padding: 0.3rem 0.5rem;
    font-size: 0.8rem;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    cursor: pointer;
    color: var(--fg-secondary);
  }
  .small-action {
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
  }
  .small-action.subtle {
    opacity: 0.92;
  }
  code {
    background: var(--bg-inset);
    border: 1px solid var(--border);
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    font-family: ui-monospace, monospace;
    font-size: 0.85em;
  }
  .activity-card {
    margin-top: 1rem;
  }
  .activity-hint {
    margin: 0 0 0.6rem;
  }
  .activity {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 420px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  .activity li {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.45rem 0.6rem;
    background: var(--bg-inset);
  }
  .activity li.bad {
    border-color: var(--danger-text);
  }
  .act-head {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .act-time {
    color: var(--fg-secondary);
    font-size: 0.75rem;
    white-space: nowrap;
  }
  .act-src {
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--fg-secondary);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0 0.35rem;
  }
  .act-src.bad {
    color: var(--danger-text);
    border-color: var(--danger-text);
  }
  .act-cmd {
    font-size: 0.8rem;
    word-break: break-all;
  }
  .act-out {
    margin: 0.35rem 0 0;
    font-family: ui-monospace, monospace;
    font-size: 0.76rem;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 160px;
    overflow: auto;
    color: var(--fg-secondary);
  }
  .activity li.bad .act-out {
    color: var(--danger-text);
  }
</style>
