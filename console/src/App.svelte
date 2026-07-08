<script lang="ts">
  import { RpcClient, handshakeUrl, type ConnState } from "./lib/rpc";
  import { ConsoleApi } from "./lib/api";
  import { parseConnectParams, defaultBase } from "./lib/connect";
  import Dashboard from "./pages/Dashboard.svelte";
  import Session from "./pages/Session.svelte";
  import Inspector from "./pages/Inspector.svelte";
  import Approvals from "./pages/Approvals.svelte";
  import Journal from "./pages/Journal.svelte";
  import Replay from "./pages/Replay.svelte";
  import Settings from "./pages/Settings.svelte";

  type Tab =
    | "dashboard"
    | "session"
    | "inspector"
    | "approvals"
    | "journal"
    | "replay"
    | "settings";

  const TABS: { id: Tab; label: string }[] = [
    { id: "dashboard", label: "Dashboard" },
    { id: "session", label: "Session" },
    { id: "inspector", label: "Inspector" },
    { id: "approvals", label: "Approvals" },
    { id: "journal", label: "Journal" },
    { id: "replay", label: "Replay" },
    { id: "settings", label: "Settings" },
  ];

  const urlParams = parseConnectParams(
    typeof location !== "undefined" ? location.search : "",
  );
  const pageBase =
    typeof location !== "undefined"
      ? defaultBase(location.protocol, location.host)
      : defaultBase("http:", "");

  let base = $state(urlParams.base ?? pageBase);
  let token = $state(urlParams.token ?? "");
  let tab = $state<Tab>("dashboard");
  let conn = $state<ConnState>("idle");
  let error = $state<string | null>(null);
  let client = $state<RpcClient | null>(null);
  let api = $state<ConsoleApi | null>(null);
  let self = $state("user:?");
  /** 递增以提示子页面在收到 evt 通知时刷新。 */
  let pulse = $state(0);
  /** 最近事件（Inspector 事件流）。 */
  let events = $state<{ seq: number; topic: string; text: string }[]>([]);

  async function connect() {
    error = null;
    try {
      const c = new RpcClient(handshakeUrl(base, token.trim()));
      c.onState((s) => (conn = s));
      c.onEvent((_method, params) => {
        pulse += 1;
        const p = params as { event?: { seq?: number; topic?: string } } | null;
        const ev = p?.event;
        if (ev && typeof ev.seq === "number") {
          events = [
            ...events.slice(-99),
            { seq: ev.seq, topic: String(ev.topic ?? "?"), text: JSON.stringify(ev) },
          ];
        }
      });
      await c.connect();
      const a = new ConsoleApi(c);
      // 连接级订阅：全部主题（gateway 会话语义），驱动 pulse 与事件流
      await a.subscribe();
      try {
        self = (await a.capList()).subject;
      } catch {
        self = "user:?";
      }
      client = c;
      api = a;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      conn = "closed";
    }
  }

  function disconnect() {
    client?.close();
    client = null;
    api = null;
    events = [];
  }

  // ?token=…&connect=1 → 自动连接（本地/e2e 便利）
  $effect(() => {
    if (urlParams.auto && !client && conn === "idle") connect();
  });
</script>

<div class="app">
  <header class="topbar">
    <span class="brand">Scoot<span class="dot">·</span>Lens Console</span>
    {#if api}
      <nav class="tabs">
        {#each TABS as t (t.id)}
          <button class:active={tab === t.id} onclick={() => (tab = t.id)} data-testid="tab-{t.id}">
            {t.label}
          </button>
        {/each}
      </nav>
    {/if}
    <span class="spacer"></span>
    {#if api}
      <span class="mono muted">{self}</span>
    {/if}
    <span class="pill {conn}"><span class="led"></span>{conn}</span>
    {#if api}
      <button class="primary" onclick={disconnect}>Disconnect</button>
    {/if}
  </header>

  <main class="content">
    {#if error}
      <div class="error">{error}</div>
    {/if}

    {#if !api}
      <div class="card" style="max-width: 520px; margin: 40px auto;">
        <h3>连接内核网关</h3>
        <div style="display:flex; flex-direction:column; gap:10px;">
          <label>
            <div class="muted">Gateway 基址</div>
            <input bind:value={base} style="width:100%" />
          </label>
          <label>
            <div class="muted">Capability 令牌（slt1…）</div>
            <input
              class="token"
              style="width:100%"
              bind:value={token}
              placeholder="slt1.xxxxx.yyyyy"
            />
          </label>
          <button
            class="primary"
            disabled={!token.trim() || conn === "connecting"}
            onclick={connect}
          >
            {conn === "connecting" ? "连接中…" : "连接"}
          </button>
        </div>
      </div>
    {:else if tab === "dashboard"}
      <Dashboard {api} {pulse} />
    {:else if tab === "session"}
      <Session {api} {pulse} {self} />
    {:else if tab === "inspector"}
      <Inspector {api} {pulse} {events} />
    {:else if tab === "approvals"}
      <Approvals {api} {pulse} />
    {:else if tab === "journal"}
      <Journal {api} {pulse} />
    {:else if tab === "replay"}
      <Replay {api} {pulse} />
    {:else}
      <Settings {api} {pulse} />
    {/if}
  </main>
</div>
