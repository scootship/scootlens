<script lang="ts">
  import type { ConsoleApi } from "../lib/api";
  import { scopeLabel } from "../lib/format";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  let self = $state<{ subject: string; scopes: string[] } | null>(null);
  let grantSubject = $state("");
  let grantScope = $state("");
  let vaultName = $state("");
  let vaultSecret = $state("");
  let vaultRef = $state<string | null>(null);
  let netRules = $state("");
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);

  function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function load() {
    try {
      self = await api.capList();
      const r = (await api.netRulesGet()) as { rules?: unknown };
      netRules = JSON.stringify(r.rules ?? { default: "allow", rules: [] }, null, 2);
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function act(fn: () => Promise<unknown>, done: string) {
    notice = null;
    error = null;
    try {
      await fn();
      notice = done;
      await load();
    } catch (e) {
      error = message(e);
    }
  }

  function grant() {
    const s = grantSubject.trim();
    const sc = grantScope.trim();
    if (!s || !sc) return;
    act(() => api.capGrant(s, sc), `已授予 ${s} ← ${sc}`);
  }

  function revoke() {
    const s = grantSubject.trim();
    const sc = grantScope.trim();
    if (!s || !sc) return;
    act(() => api.capRevoke(s, sc), `已撤销 ${s} ✗ ${sc}`);
  }

  function writeVault() {
    const name = vaultName.trim();
    if (!name || !vaultSecret) return;
    act(async () => {
      await api.vaultWrite(name, vaultSecret);
      vaultRef = name;
      vaultSecret = "";
      vaultName = "";
    }, `凭据已写入 vault（只写不读）`);
  }

  function applyNetRules() {
    act(async () => {
      const parsed = JSON.parse(netRules) as unknown;
      await api.netRulesSet(parsed);
    }, "全局网络规则已生效");
  }

  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <h2>Settings</h2>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}
{#if notice}
  <div class="notice" data-testid="settings-notice">{notice}</div>
{/if}

<div class="grid">
  <div class="card">
    <h3>本会话令牌</h3>
    {#if self}
      <div class="mono">{self.subject}</div>
      <div style="margin-top:8px; display:flex; flex-wrap:wrap; gap:6px;">
        {#each self.scopes as s (s)}
          <span class="tag info mono">{scopeLabel(s)}</span>
        {/each}
      </div>
    {:else}
      <div class="empty">加载中…</div>
    {/if}
  </div>

  <div class="card">
    <h3>令牌管理 <small class="muted">cap.grant / cap.revoke（动态授权）</small></h3>
    <div style="display:flex; flex-direction:column; gap:8px;">
      <input placeholder="主体，如 agent:ops-bot-1" bind:value={grantSubject} data-testid="grant-subject" />
      <input placeholder="作用域，如 nav@*.example.com" bind:value={grantScope} data-testid="grant-scope" />
      <div class="toolbar">
        <button class="primary" onclick={grant} data-testid="grant-btn">授予</button>
        <button class="danger" onclick={revoke} data-testid="revoke-btn">撤销</button>
      </div>
      <small class="muted">令牌签发在守护进程侧（`scootlensd --issue`）；此处调整已发主体的有效作用域。</small>
    </div>
  </div>

  <div class="card">
    <h3>vault 写入 <small class="muted">单向；Agent 经 vault_ref 使用</small></h3>
    <div style="display:flex; flex-direction:column; gap:8px;">
      <input placeholder="凭据名，如 gh-password" bind:value={vaultName} data-testid="vault-name" />
      <input class="token" type="password" placeholder="凭据值（写入后不可读回）" bind:value={vaultSecret} data-testid="vault-secret" />
      <div class="toolbar">
        <button class="primary" onclick={writeVault} data-testid="vault-write">写入 vault</button>
        {#if vaultRef}
          <span class="tag ok mono" data-testid="vault-ref">vault_ref: {vaultRef}</span>
        {/if}
      </div>
    </div>
  </div>

  <div class="card">
    <h3>全局网络规则 <small class="muted">net.rules.set（default + rules[]）</small></h3>
    <textarea rows="8" bind:value={netRules} class="mono" data-testid="net-rules"></textarea>
    <div class="toolbar">
      <button class="primary" onclick={applyNetRules} data-testid="net-rules-apply">应用</button>
    </div>
  </div>
</div>
