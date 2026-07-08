<script lang="ts">
  import { RpcClient, handshakeUrl, type ConnState } from "./lib/rpc";
  import { ConsoleApi } from "./lib/api";
  import Dashboard from "./pages/Dashboard.svelte";
  import Approvals from "./pages/Approvals.svelte";
  import Journal from "./pages/Journal.svelte";

  type Tab = "dashboard" | "approvals" | "journal";

  const defaultBase =
    typeof location !== "undefined"
      ? `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}`
      : "ws://127.0.0.1:8787";

  let base = $state(defaultBase);
  let token = $state("");
  let tab = $state<Tab>("dashboard");
  let conn = $state<ConnState>("idle");
  let error = $state<string | null>(null);
  let client = $state<RpcClient | null>(null);
  let api = $state<ConsoleApi | null>(null);
  /** 递增以提示子页面在收到 evt 通知时刷新。 */
  let pulse = $state(0);

  async function connect() {
    error = null;
    try {
      const c = new RpcClient(handshakeUrl(base, token.trim()));
      c.onState((s) => (conn = s));
      c.onEvent(() => (pulse += 1));
      await c.connect();
      client = c;
      api = new ConsoleApi(c);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      conn = "closed";
    }
  }

  function disconnect() {
    client?.close();
    client = null;
    api = null;
  }
</script>

<div class="app">
  <header class="topbar">
    <span class="brand">Scoot<span class="dot">·</span>Lens Console</span>
    {#if api}
      <nav class="tabs">
        <button class:active={tab === "dashboard"} onclick={() => (tab = "dashboard")}>
          Dashboard
        </button>
        <button class:active={tab === "approvals"} onclick={() => (tab = "approvals")}>
          Approvals
        </button>
        <button class:active={tab === "journal"} onclick={() => (tab = "journal")}>
          Journal
        </button>
      </nav>
    {/if}
    <span class="spacer"></span>
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
    {:else if tab === "approvals"}
      <Approvals {api} {pulse} />
    {:else}
      <Journal {api} {pulse} />
    {/if}
  </main>
</div>
