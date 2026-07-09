<script lang="ts">
  import { RpcClient, handshakeUrl, type ConnState } from "./lib/rpc";
  import { ConsoleApi } from "./lib/api";
  import { parseConnectParams, defaultBase } from "./lib/connect";
  import {
    fetchProviders,
    fetchMe,
    loginPassword,
    logout as authLogout,
    loginErrorMessage,
    type AuthProviders,
  } from "./lib/auth";
  import {
    listAutoApprove,
    matchesAutoApprove,
    parseCapRequest,
    toggleAutoApprove,
  } from "./lib/autoapprove";
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

  /** 侧栏导航（分组 + 图标）。data-testid 维持 `tab-*` 兼容 e2e。 */
  const NAV: { label: string; items: { id: Tab; label: string; icon: string }[] }[] = [
    {
      label: "概览",
      items: [
        {
          id: "dashboard",
          label: "Dashboard",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="1.5" y="1.5" width="5.5" height="5.5" rx="1"/><rect x="9" y="1.5" width="5.5" height="5.5" rx="1"/><rect x="1.5" y="9" width="5.5" height="5.5" rx="1"/><rect x="9" y="9" width="5.5" height="5.5" rx="1"/></svg>',
        },
      ],
    },
    {
      label: "会话",
      items: [
        {
          id: "session",
          label: "Session",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="1.5" y="2.5" width="13" height="9" rx="1.5"/><path d="M5.5 14h5M8 11.5V14"/></svg>',
        },
        {
          id: "inspector",
          label: "Inspector",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 4 4"/></svg>',
        },
        {
          id: "replay",
          label: "Replay",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="6.5"/><path d="M6.5 5.5 11 8l-4.5 2.5z" fill="currentColor" stroke="none"/></svg>',
        },
      ],
    },
    {
      label: "治理",
      items: [
        {
          id: "approvals",
          label: "Approvals",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M8 1.5 14 4v4c0 3.5-2.5 6-6 6.5C4.5 14 2 11.5 2 8V4z"/><path d="m5.5 8 1.8 1.8L11 6"/></svg>',
        },
        {
          id: "journal",
          label: "Journal",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M3 1.5h8.5L14 4v10.5H3z"/><path d="M5.5 6h5M5.5 9h5M5.5 12h3"/></svg>',
        },
      ],
    },
    {
      label: "配置",
      items: [
        {
          id: "settings",
          label: "Settings",
          icon: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M12.6 3.4l-1.4 1.4M4.8 11.2l-1.4 1.4"/></svg>',
        },
      ],
    },
  ];

  const PAGE_TITLES: Record<Tab, string> = {
    dashboard: "Dashboard",
    session: "Session",
    inspector: "Inspector",
    approvals: "Approvals",
    journal: "Journal",
    replay: "Replay",
    settings: "Settings",
  };

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
  /** 自动审批：勾选的作用域族（Approvals 页维护；这里消费）。 */
  let autoApprove = $state<Set<string>>(listAutoApprove());
  /** 最近一次自动批准的说明（Approvals 页展示）。 */
  let autoApprovedNote = $state<string | null>(null);

  /** cap.request 事件命中勾选集合 → 自动批准（不 remember：范围锁在本次勾选期）。 */
  function maybeAutoApprove(a: ConsoleApi, text: string) {
    const req = parseCapRequest(text);
    if (!req) return;
    const rule = matchesAutoApprove(req.scope, autoApprove);
    if (!rule) return;
    a.approve(req.approvalId, "allow", false)
      .then(() => {
        autoApprovedNote = `已自动批准 ${req.method}（${req.scope} · 规则 ${rule}）`;
      })
      .catch(() => {
        // 竞态（已被他人处理）或权限不足：留给人工收件箱，不打扰
      });
  }

  // ---------- 登录状态 ----------
  let providers = $state<AuthProviders>({ password: false, microsoft: false });
  /** cookie 会话主体（null = 未登录）。 */
  let cookieSession = $state<string | null>(null);
  let authChecked = $state(false);
  let loginUser = $state("admin");
  let loginPass = $state("");
  let loginBusy = $state(false);
  /** OAuth 回调失败提示（?login_error=…）。 */
  let oauthError = $state<string | null>(
    typeof location !== "undefined" ? loginErrorMessage(location.search) : null,
  );

  // ---------- 布局 ----------
  let sidebarOpen = $state(false);
  let pendingCount = $state(0);

  /** 建立 WS 连接：token 非空走令牌握手，否则用登录会话 cookie。 */
  async function connect(useToken: string) {
    error = null;
    try {
      const c = new RpcClient(handshakeUrl(base, useToken));
      let liveApi: ConsoleApi | null = null;
      c.onState((s) => (conn = s));
      c.onEvent((_method, params) => {
        pulse += 1;
        const p = params as { event?: { seq?: number; topic?: string } } | null;
        const ev = p?.event;
        if (ev && typeof ev.seq === "number") {
          const text = JSON.stringify(ev);
          events = [
            ...events.slice(-199),
            { seq: ev.seq, topic: String(ev.topic ?? "?"), text },
          ];
          if (liveApi && ev.topic === "cap.request") maybeAutoApprove(liveApi, text);
        }
      });
      await c.connect();
      const a = new ConsoleApi(c);
      liveApi = a;
      // 连接级订阅：全部主题（gateway 会话语义），驱动 pulse 与事件流
      await a.subscribe();
      try {
        self = (await a.capList()).subject;
      } catch {
        self = cookieSession ?? "user:?";
      }
      client = c;
      api = a;
    } catch (e) {
      error =
        e instanceof Error
          ? e.message
          : "连接失败（网关拒绝握手或不可达）";
      conn = "closed";
    }
  }

  async function submitLogin(ev: SubmitEvent) {
    ev.preventDefault();
    if (!loginUser.trim() || !loginPass) return;
    loginBusy = true;
    error = null;
    oauthError = null;
    try {
      cookieSession = await loginPassword(loginUser.trim(), loginPass);
      loginPass = "";
      await connect("");
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loginBusy = false;
    }
  }

  function connectWithToken() {
    if (!token.trim()) return;
    cookieSession = null;
    connect(token.trim());
  }

  function disconnect() {
    client?.close();
    client = null;
    api = null;
    events = [];
    pendingCount = 0;
    tab = "dashboard";
  }

  async function signOut() {
    if (cookieSession) {
      await authLogout();
      cookieSession = null;
    }
    disconnect();
  }

  function pickTab(id: Tab) {
    tab = id;
    sidebarOpen = false;
  }

  // 初始化：探测登录方式 + 已有会话；?token=…&connect=1 维持自动令牌连接（e2e/Agent 便利）
  $effect(() => {
    if (authChecked) return;
    authChecked = true;
    (async () => {
      providers = await fetchProviders();
      if (urlParams.auto) {
        connectWithToken();
        return;
      }
      const me = await fetchMe();
      if (me) {
        cookieSession = me;
        await connect("");
      }
    })();
  });

  // 审批徽标：事件驱动刷新（cap.pending 需要 admin 作用域；无权限时静默为 0）
  $effect(() => {
    void pulse;
    const a = api;
    if (!a) return;
    a.pending()
      .then((items) => (pendingCount = items.length))
      .catch(() => (pendingCount = 0));
  });
</script>

{#if !api}
  <div class="login-wrap">
    <div class="login-card">
      <div class="brand">
        <span class="brand-mark">SL</span>
        <span>Scoot<span class="dot">·</span>Lens Console</span>
      </div>
      <div class="login-sub">Capability 沙箱内核 · 管理控制台</div>

      {#if oauthError}
        <div class="error" data-testid="login-error">{oauthError}</div>
      {/if}
      {#if error}
        <div class="error">{error}</div>
      {/if}

      {#if providers.password}
        <form class="login-form" onsubmit={submitLogin}>
          <label class="field">
            <span class="field-label">用户名</span>
            <input
              bind:value={loginUser}
              autocomplete="username"
              data-testid="login-user"
            />
          </label>
          <label class="field">
            <span class="field-label">密码</span>
            <input
              type="password"
              bind:value={loginPass}
              autocomplete="current-password"
              data-testid="login-pass"
            />
          </label>
          <button
            class="primary"
            type="submit"
            disabled={loginBusy || !loginUser.trim() || !loginPass}
            data-testid="login-submit"
          >
            {loginBusy ? "登录中…" : "登录"}
          </button>
        </form>
      {/if}

      {#if providers.microsoft}
        {#if providers.password}
          <div class="login-divider">或</div>
        {/if}
        <a class="ms-btn" href="/auth/ms/login" data-testid="login-ms">
          <svg width="15" height="15" viewBox="0 0 16 16" aria-hidden="true">
            <rect x="0.5" y="0.5" width="7" height="7" fill="#f25022" />
            <rect x="8.5" y="0.5" width="7" height="7" fill="#7fba00" />
            <rect x="0.5" y="8.5" width="7" height="7" fill="#00a4ef" />
            <rect x="8.5" y="8.5" width="7" height="7" fill="#ffb900" />
          </svg>
          使用 Microsoft 登录
        </a>
      {/if}

      <details class="login-advanced" open={!providers.password && !providers.microsoft}>
        <summary>使用 Capability 令牌连接（Agent / 高级）</summary>
        <div class="login-form">
          <label class="field">
            <span class="field-label">Gateway 基址</span>
            <input bind:value={base} />
          </label>
          <label class="field">
            <span class="field-label">Capability 令牌（slt1…）</span>
            <input class="token" style="width:100%" bind:value={token} placeholder="slt1.xxxxx.yyyyy" />
          </label>
          <button
            class="primary"
            disabled={!token.trim() || conn === "connecting"}
            onclick={connectWithToken}
          >
            {conn === "connecting" ? "连接中…" : "连接"}
          </button>
          <p class="hint" style="margin:0">
            令牌连接面向自动化接入；日常管理请使用登录方式，避免令牌进入浏览器历史。
          </p>
        </div>
      </details>
    </div>
  </div>
{:else}
  <div class="shell">
    {#if sidebarOpen}
      <button class="backdrop" aria-label="关闭菜单" onclick={() => (sidebarOpen = false)}
      ></button>
    {/if}
    <aside class="sidebar" class:open={sidebarOpen}>
      <div class="brand">
        <span class="brand-mark">SL</span>
        <span>Scoot<span class="dot">·</span>Lens</span>
      </div>
      <nav class="nav">
        {#each NAV as group (group.label)}
          <div class="nav-group">
            <div class="nav-label">{group.label}</div>
            {#each group.items as item (item.id)}
              <button
                class:active={tab === item.id}
                onclick={() => pickTab(item.id)}
                data-testid="tab-{item.id}"
              >
                <span class="icon">{@html item.icon}</span>
                {item.label}
                {#if item.id === "approvals" && pendingCount > 0}
                  <span class="badge" data-testid="approvals-badge">{pendingCount}</span>
                {/if}
              </button>
            {/each}
          </div>
        {/each}
      </nav>
      <div class="sidebar-foot">
        <div class="who">
          <span class="mono" title={self}>{self}</span>
        </div>
        <button class="ghost" onclick={signOut}>
          {cookieSession ? "退出登录" : "断开连接"}
        </button>
      </div>
    </aside>

    <div class="main">
      <header class="topbar">
        <button
          class="hamburger"
          aria-label="打开菜单"
          onclick={() => (sidebarOpen = !sidebarOpen)}>☰</button
        >
        <span class="page-title">{PAGE_TITLES[tab]}</span>
        <span class="spacer"></span>
        <span class="pill {conn}"><span class="led"></span>{conn}</span>
      </header>

      <main class="content">
        {#if error}
          <div class="error">{error}</div>
        {/if}

        {#if tab === "dashboard"}
          <Dashboard {api} {pulse} />
        {:else if tab === "session"}
          <Session {api} {pulse} {self} />
        {:else if tab === "inspector"}
          <Inspector {api} {pulse} {events} />
        {:else if tab === "approvals"}
          <Approvals
            {api}
            {pulse}
            enabled={autoApprove}
            note={autoApprovedNote}
            onToggle={(id, on) => (autoApprove = toggleAutoApprove(id, on))}
          />
        {:else if tab === "journal"}
          <Journal {api} {pulse} />
        {:else if tab === "replay"}
          <Replay {api} {pulse} />
        {:else}
          <Settings {api} {pulse} />
        {/if}
      </main>
    </div>
  </div>
{/if}
