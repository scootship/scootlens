<script lang="ts">
  import type { ConsoleApi } from "../lib/api";
  import { scopeLabel } from "../lib/format";
  import { buildStateBundle } from "../lib/cookies";
  import { listProfiles, rememberProfile } from "../lib/profiles";
  import {
    forgetCredential,
    listCredentials,
    saveCredential,
    type CredentialProfile,
  } from "../lib/credentials";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  type SubTab = "tokens" | "network" | "vault" | "session";
  let sub = $state<SubTab>("tokens");
  const SUBTABS: { id: SubTab; label: string }[] = [
    { id: "tokens", label: "令牌 / 权限" },
    { id: "network", label: "网络规则" },
    { id: "vault", label: "凭据 Vault" },
    { id: "session", label: "登录会话" },
  ];

  let self = $state<{ subject: string; scopes: string[] } | null>(null);
  let grantSubject = $state("");
  let grantScope = $state("");
  let vaultName = $state("");
  let vaultSecret = $state("");
  let vaultRef = $state<string | null>(null);
  let vaultNames = $state<string[]>([]);
  let credentialLabel = $state("");
  let credentialOrigin = $state("");
  let credentialUsernameRef = $state("");
  let credentialPasswordRef = $state("");
  let credentialLoginUrl = $state("");
  let credentials = $state<CredentialProfile[]>(listCredentials());
  let netRules = $state("");
  let importProfile = $state("");
  let importCookies = $state("");
  let importStorage = $state("");
  let knownProfiles = $state<string[]>(listProfiles());
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);

  // 粘贴即时预览：解析成功则显示条数，失败则显示原因（不阻塞输入）。
  const importPreview = $derived.by(() => {
    if (!importCookies.trim()) return null;
    try {
      const r = buildStateBundle(importCookies, importStorage);
      return { ok: true as const, ...r };
    } catch (e) {
      return { ok: false as const, message: e instanceof Error ? e.message : String(e) };
    }
  });

  function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function load() {
    try {
      self = await api.capList();
      const r = (await api.netRulesGet()) as { rules?: unknown };
      netRules = JSON.stringify(r.rules ?? { default: "allow", rules: [] }, null, 2);
      try {
        vaultNames = await api.vaultList();
      } catch {
        vaultNames = [];
      }
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
      vaultNames = await api.vaultList().catch(() => vaultNames);
      vaultRef = name;
      vaultSecret = "";
      vaultName = "";
    }, `凭据已写入 vault（只写不读）`);
  }

  function saveCredentialProfile() {
    notice = null;
    error = null;
    try {
      credentials = saveCredential({
        label: credentialLabel,
        origin: credentialOrigin,
        usernameRef: credentialUsernameRef,
        passwordRef: credentialPasswordRef,
        loginUrl: credentialLoginUrl,
      });
      credentialLabel = "";
      credentialOrigin = "";
      credentialUsernameRef = "";
      credentialPasswordRef = "";
      credentialLoginUrl = "";
      notice = "凭据绑定已保存";
    } catch (e) {
      error = message(e);
    }
  }

  function deleteCredentialProfile(id: string) {
    credentials = forgetCredential(id);
    notice = "凭据绑定已删除";
    error = null;
  }

  function applyNetRules() {
    act(async () => {
      const parsed = JSON.parse(netRules) as unknown;
      await api.netRulesSet(parsed);
    }, "全局网络规则已生效");
  }

  function importSession() {
    const profile = importProfile.trim();
    if (!profile) {
      error = "请填写目标 profile 名";
      return;
    }
    if (!importCookies.trim()) {
      error = "请粘贴 cookie 导出 JSON";
      return;
    }
    act(async () => {
      const { bundle, cookies, httpOnly, storage } = buildStateBundle(
        importCookies,
        importStorage,
      );
      await api.stateImport(profile, bundle);
      knownProfiles = rememberProfile(profile);
      importCookies = "";
      importStorage = "";
      return { cookies, httpOnly, storage };
    }, `已导入 profile「${importProfile.trim()}」→ 去 Session 页从下拉选此 profile 点「新开会话」，即带登录态并可接管`);
  }

  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <h2>Settings</h2>
  <nav class="tabs subtabs">
    {#each SUBTABS as t (t.id)}
      <button class:active={sub === t.id} onclick={() => (sub = t.id)} data-testid="subtab-{t.id}">
        {t.label}
      </button>
    {/each}
  </nav>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}
{#if notice}
  <div class="notice" data-testid="settings-notice">{notice}</div>
{/if}

{#if sub === "tokens"}
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
  </div>
{:else if sub === "network"}
  <div class="grid">
    <div class="card" style="grid-column: 1 / -1;">
      <h3>全局网络规则 <small class="muted">net.rules.set（default + rules[]）</small></h3>
      <textarea rows="10" bind:value={netRules} class="mono" data-testid="net-rules"></textarea>
      <div class="toolbar">
        <button class="primary" onclick={applyNetRules} data-testid="net-rules-apply">应用</button>
      </div>
    </div>
  </div>
{:else if sub === "vault"}
  <div class="grid">
    <div class="card">
      <h3>vault 写入 <small class="muted">单向；Agent 经 vault_ref 使用</small></h3>
      <div style="display:flex; flex-direction:column; gap:8px; max-width:520px;">
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
      <h3>站点凭据绑定 <small class="muted">origin → vault_ref</small></h3>
      <div style="display:flex; flex-direction:column; gap:8px; max-width:560px;">
        <input placeholder="名称，如 GitHub 主账号" bind:value={credentialLabel} data-testid="credential-label" />
        <input placeholder="域名 / origin，如 github.com 或 *.corp.test" bind:value={credentialOrigin} data-testid="credential-origin" />
        <input
          class="mono"
          placeholder="用户名 vault_ref，如 gh-username"
          bind:value={credentialUsernameRef}
          list="vault-names"
          data-testid="credential-username-ref"
        />
        <input
          class="mono"
          placeholder="密码 vault_ref，如 gh-password"
          bind:value={credentialPasswordRef}
          list="vault-names"
          data-testid="credential-password-ref"
        />
        <input placeholder="登录页 URL（可选）" bind:value={credentialLoginUrl} data-testid="credential-login-url" />
        {#if vaultNames.length}
          <datalist id="vault-names">
            {#each vaultNames as n (n)}
              <option value={n}></option>
            {/each}
          </datalist>
        {/if}
        <div class="toolbar">
          <button class="primary" onclick={saveCredentialProfile} data-testid="credential-save">保存绑定</button>
        </div>
        <small class="muted">只保存域名与 vault_ref；Session 页仅在当前 URL 命中该 origin 时显示填充动作。</small>
      </div>
    </div>

    <div class="card" style="grid-column: 1 / -1;">
      <h3>已保存绑定</h3>
      {#if credentials.length === 0}
        <div class="empty">暂无凭据绑定</div>
      {:else}
        <table>
          <thead><tr><th>名称</th><th>origin</th><th>vault_ref</th><th>登录页</th><th></th></tr></thead>
          <tbody>
            {#each credentials as c (c.id)}
              <tr>
                <td>{c.label}</td>
                <td class="mono">{c.origin}</td>
                <td class="mono">{c.usernameRef} / {c.passwordRef}</td>
                <td class="mono">{c.loginUrl ?? "—"}</td>
                <td class="row-actions">
                  <button class="danger" onclick={() => deleteCredentialProfile(c.id)} data-testid="credential-delete-{c.id}">
                    删除
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
{:else if sub === "session"}
  <div class="grid">
    <div class="card" style="grid-column: 1 / -1;">
      <h3>导入登录会话 <small class="muted">粘贴 cookie → state.import → profile 复用</small></h3>
      <p class="muted" style="margin:0 0 8px; font-size:12px; line-height:1.5;">
        浏览器安全禁止网页读取其它站点的 cookie，所以要先从扩展导出：在你已登录的
        Chrome 里装 <b>Cookie-Editor</b> → 打开目标站 → 点扩展 → <b>Export</b>（JSON）→ 粘贴到下面。
        httpOnly 的会话 cookie 只能这样拿到（<code>document.cookie</code> 读不到）。
        localStorage 可选：控制台跑 <code>JSON.stringify(Object.entries(localStorage))</code> 复制粘贴。
      </p>
      <div style="display:flex; flex-direction:column; gap:8px;">
        <input
          placeholder="目标 profile 名，如 github"
          bind:value={importProfile}
          list="known-profiles"
          data-testid="import-profile"
        />
        {#if knownProfiles.length}
          <datalist id="known-profiles">
            {#each knownProfiles as p (p)}
              <option value={p}></option>
            {/each}
          </datalist>
        {/if}
        <textarea
          rows="6"
          class="mono"
          placeholder="粘贴 Cookie-Editor 导出的 JSON（数组）…"
          bind:value={importCookies}
          data-testid="import-cookies"
        ></textarea>
        <textarea
          rows="3"
          class="mono"
          placeholder="可选：localStorage JSON（对象或 [[键,值]]）…"
          bind:value={importStorage}
          data-testid="import-storage"
        ></textarea>
        <div class="toolbar">
          <button
            class="primary"
            onclick={importSession}
            disabled={!importPreview?.ok}
            data-testid="import-session"
          >
            导入会话
          </button>
          {#if importPreview?.ok}
            <span class="tag ok" data-testid="import-preview">
              {importPreview.cookies} cookie（httpOnly {importPreview.httpOnly}）
              {#if importPreview.storage}· {importPreview.storage} storage{/if}
            </span>
          {:else if importPreview}
            <span class="tag danger" data-testid="import-preview">{importPreview.message}</span>
          {/if}
        </div>
        <small class="muted">
          导入是敏感操作：用 admin 令牌打开的 console 会自动放行；普通令牌则到
          Approvals 标签点 Allow。之后到 <strong>Session 页</strong>从下拉选此 profile 点「新开会话」，即带登录态并可接管。
        </small>
      </div>
    </div>
  </div>
{/if}
