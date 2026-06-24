<script lang="ts">
  import { api } from "$lib/api";

  let { serial }: { serial: string } = $props();

  let command = $state("");
  let output = $state("");
  let ok = $state(true);
  let busy = $state(false);

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
</style>
