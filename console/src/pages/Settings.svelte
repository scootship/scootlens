<script lang="ts">
  import type { ConsoleApi, ProfileEntryDigest } from "../lib/api";
  import { scopeLabel } from "../lib/format";
  import { buildStateBundle } from "../lib/cookies";
  import { forgetProfile, listProfiles, rememberProfile } from "../lib/profiles";
  import {
    forgetCredential,
    listCredentials,
    saveCredential,
    type CredentialProfile,
  } from "../lib/credentials";
  import { friendlyError } from "../lib/errors";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  type SubTab = "tokens" | "network" | "vault" | "bindings" | "session";
  let sub = $state<SubTab>("tokens");
  const SUBTABS: { id: SubTab; label: string }[] = [
    { id: "tokens", label: "令牌 / 权限" },
    { id: "network", label: "网络规则" },
    { id: "vault", label: "凭据 Vault" },
    { id: "bindings", label: "站点绑定" },
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
  /** 内核侧权威列表（state.list namespace=profiles）。 */
  let importedProfiles = $state<string[]>([]);
  /** 当前展开摘要的 profile；digest 只含元数据，值永不回流。 */
  let digestFor = $state<string | null>(null);
  let digest = $state<ProfileEntryDigest[]>([]);
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
      try {
        importedProfiles = await api.profileList();
        // 内核列表并入本地记忆，让「新开会话」下拉总能看到已导入的 profile
        for (const p of importedProfiles) knownProfiles = rememberProfile(p);
      } catch {
        importedProfiles = [];
      }
      error = null;
    } catch (e) {
      error = friendlyError(e);
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
      error = friendlyError(e);
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

  /** 删除 vault 凭据（值从未回流过；历史 journal 的脱敏不回收）。 */
  function deleteVaultName(name: string) {
    act(async () => {
      await api.vaultDelete(name);
      vaultNames = await api.vaultList().catch(() => vaultNames.filter((n) => n !== name));
      if (vaultRef === name) vaultRef = null;
    }, `已删除凭据「${name}」`);
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
      error = friendlyError(e);
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

  /** 查看 profile 摘要（state.read 只回元数据，值绝不回流）。 */
  async function viewProfile(name: string) {
    notice = null;
    error = null;
    if (digestFor === name) {
      digestFor = null;
      digest = [];
      return;
    }
    try {
      digest = await api.profileDigest(name);
      digestFor = name;
    } catch (e) {
      error = friendlyError(e);
    }
  }

  /** 整删 profile（只清存储，运行中的会话不受影响）。 */
  function deleteProfile(name: string) {
    act(async () => {
      await api.profileDelete(name);
      knownProfiles = forgetProfile(name);
      if (digestFor === name) {
        digestFor = null;
        digest = [];
      }
    }, `已删除 profile「${name}」`);
  }

  /** 删除 profile 内单条 entry，并刷新摘要。 */
  function deleteProfileEntry(name: string, entry: string) {
    act(async () => {
      await api.profileDelete(name, entry);
      digest = await api.profileDigest(name).catch(() => []);
    }, `已从「${name}」删除 ${entry}`);
  }

  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
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
  <div class="settings-grid">
    <div class="card">
      <h3>本会话身份</h3>
      <p class="card-desc">当前连接的主体与生效作用域（cap.list）。</p>
      {#if self}
        <div class="mono subject">{self.subject}</div>
        <div class="scope-list">
          {#each self.scopes as s (s)}
            <span class="tag info mono">{scopeLabel(s)}</span>
          {/each}
        </div>
      {:else}
        <div class="empty">加载中…</div>
      {/if}
    </div>

    <div class="card">
      <h3>动态授权 <small class="muted">cap.grant / cap.revoke</small></h3>
      <p class="card-desc">
        调整已签发主体的有效作用域。令牌本身在守护进程侧签发
        （<code>scootlensd --issue</code>），此处只增减作用域。
      </p>
      <div class="form-col">
        <label class="field">
          <span class="field-label">主体</span>
          <input placeholder="如 agent:ops-bot-1" bind:value={grantSubject} data-testid="grant-subject" />
        </label>
        <label class="field">
          <span class="field-label">作用域</span>
          <input placeholder="如 nav@*.example.com" bind:value={grantScope} data-testid="grant-scope" />
        </label>
        <div class="toolbar" style="margin:2px 0 0">
          <button class="primary" onclick={grant} data-testid="grant-btn">授予</button>
          <button class="danger" onclick={revoke} data-testid="revoke-btn">撤销</button>
        </div>
      </div>
    </div>
  </div>
{:else if sub === "network"}
  <div class="card">
    <h3>全局网络规则 <small class="muted">net.rules.set</small></h3>
    <p class="card-desc">
      JSON 形如 <code>{"{"} "default": "allow", "rules": [...] {"}"}</code>；
      按序首条命中生效，无命中走 default。proc 级规则优先于全局。
    </p>
    <textarea rows="12" bind:value={netRules} class="mono" data-testid="net-rules"></textarea>
    <div class="toolbar" style="margin:10px 0 0">
      <button class="primary" onclick={applyNetRules} data-testid="net-rules-apply">应用</button>
    </div>
  </div>
{:else if sub === "vault"}
  <div class="card" style="max-width:720px">
    <h3>写入凭据 <small class="muted">state.write · namespace=vault</small></h3>
    <p class="card-desc">
      单向保险库：写入后不可读回，Agent 只能经 <code>vault_ref</code> 句柄在
      受控动作里引用（如表单填充），明文永不回流。
    </p>
    <div class="form-col">
      <label class="field">
        <span class="field-label">凭据名</span>
        <input placeholder="如 gh-password" bind:value={vaultName} data-testid="vault-name" />
      </label>
      <label class="field">
        <span class="field-label">凭据值</span>
        <input class="token" style="width:100%" type="password" placeholder="写入后不可读回" bind:value={vaultSecret} data-testid="vault-secret" />
      </label>
      <div class="toolbar" style="margin:2px 0 0">
        <button class="primary" onclick={writeVault} data-testid="vault-write">写入 vault</button>
        {#if vaultRef}
          <span class="tag ok mono" data-testid="vault-ref">vault_ref: {vaultRef}</span>
        {/if}
      </div>
    </div>
    {#if vaultNames.length}
      <table style="margin-top:12px" data-testid="vault-table">
        <thead>
          <tr><th>已存凭据（只列名，值不可读回）</th><th style="width:1%"></th></tr>
        </thead>
        <tbody>
          {#each vaultNames as n (n)}
            <tr>
              <td class="mono">{n}</td>
              <td class="row-actions">
                <button class="danger" onclick={() => deleteVaultName(n)} data-testid="vault-delete-{n}">
                  删除
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{:else if sub === "bindings"}
  <div class="card" style="max-width:720px">
    <h3>站点凭据绑定 <small class="muted">origin → vault_ref</small></h3>
    <p class="card-desc">
      保存域名与 vault 句柄的绑定；Session 页只在当前 URL 命中该 origin 时显示填充动作。
    </p>
    <div class="form-col">
      <label class="field">
        <span class="field-label">名称</span>
        <input placeholder="如 GitHub 主账号" bind:value={credentialLabel} data-testid="credential-label" />
      </label>
      <label class="field">
        <span class="field-label">域名 / origin</span>
        <input placeholder="如 github.com 或 *.corp.test" bind:value={credentialOrigin} data-testid="credential-origin" />
      </label>
      <label class="field">
        <span class="field-label">用户名 vault_ref</span>
        <input
          class="mono"
          placeholder="如 gh-username"
          bind:value={credentialUsernameRef}
          list="vault-names"
          data-testid="credential-username-ref"
        />
      </label>
      <label class="field">
        <span class="field-label">密码 vault_ref</span>
        <input
          class="mono"
          placeholder="如 gh-password"
          bind:value={credentialPasswordRef}
          list="vault-names"
          data-testid="credential-password-ref"
        />
      </label>
      <label class="field">
        <span class="field-label">登录页 URL（可选）</span>
        <input placeholder="https://github.com/login" bind:value={credentialLoginUrl} data-testid="credential-login-url" />
      </label>
      {#if vaultNames.length}
        <datalist id="vault-names">
          {#each vaultNames as n (n)}
            <option value={n}></option>
          {/each}
        </datalist>
      {/if}
      <div class="toolbar" style="margin:2px 0 0">
        <button class="primary" onclick={saveCredentialProfile} data-testid="credential-save">保存绑定</button>
      </div>
    </div>
  </div>

  <div class="card" style="max-width:720px; margin-top:14px">
    <h3>已保存绑定</h3>
    {#if credentials.length === 0}
      <div class="empty">暂无凭据绑定</div>
    {:else}
      <div class="table-scroll">
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
      </div>
    {/if}
  </div>
{:else if sub === "session"}
  <div class="card" style="max-width:720px">
    <h3>导入登录会话 <small class="muted">cookie → state.import → profile 复用</small></h3>
    <p class="card-desc">
      浏览器安全禁止网页读取其它站点的 cookie，所以要先从扩展导出：在你已登录的
      Chrome 里装 <b>Cookie-Editor</b> → 打开目标站 → 点扩展 → <b>Export</b>（JSON）→
      粘贴到下面。httpOnly 的会话 cookie 只能这样拿到（<code>document.cookie</code> 读不到）。
      localStorage 可选：控制台跑
      <code>JSON.stringify(Object.entries(localStorage))</code> 复制粘贴。
    </p>
    <div class="form-col">
      <label class="field">
        <span class="field-label">目标 profile 名</span>
        <input
          placeholder="如 github"
          bind:value={importProfile}
          list="known-profiles"
          data-testid="import-profile"
        />
      </label>
      {#if knownProfiles.length}
        <datalist id="known-profiles">
          {#each knownProfiles as p (p)}
            <option value={p}></option>
          {/each}
        </datalist>
      {/if}
      <label class="field">
        <span class="field-label">Cookie 导出 JSON</span>
        <textarea
          rows="6"
          class="mono"
          placeholder="粘贴 Cookie-Editor 导出的 JSON（数组）…"
          bind:value={importCookies}
          data-testid="import-cookies"
        ></textarea>
      </label>
      <label class="field">
        <span class="field-label">localStorage JSON（可选）</span>
        <textarea
          rows="3"
          class="mono"
          placeholder="对象或 [[键,值]] 形式…"
          bind:value={importStorage}
          data-testid="import-storage"
        ></textarea>
      </label>
      <div class="toolbar" style="margin:2px 0 0">
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
      <p class="hint" style="margin:4px 0 0">
        导入是敏感操作：用 admin 身份打开的 console 自动放行；普通令牌会进
        Approvals 待批。之后到 <strong>Session 页</strong>从下拉选此 profile
        点「新开会话」，即带登录态并可接管。
      </p>
    </div>
  </div>

  <div class="card" style="max-width:720px; margin-top:14px">
    <h3>已导入的 profiles <small class="muted">state.list / state.read / state.delete · namespace=profiles</small></h3>
    <p class="card-desc">
      导入的 cookie 是登录凭据，这里只展示<b>元数据</b>（名字/域/标志/字节数），
      值永不回流——与 vault 只列名同一原则。删除只清 profile 存储，
      运行中的会话不受影响。
    </p>
    {#if importedProfiles.length === 0}
      <div class="empty" data-testid="profiles-empty">内核里还没有已导入的 profile</div>
    {:else}
      <table data-testid="profiles-table">
        <thead>
          <tr><th>profile</th><th style="width:1%"></th></tr>
        </thead>
        <tbody>
          {#each importedProfiles as p (p)}
            <tr>
              <td class="mono">{p}</td>
              <td class="row-actions">
                <button onclick={() => viewProfile(p)} data-testid="profile-view-{p}">
                  {digestFor === p ? "收起" : "查看"}
                </button>
                <button class="danger" onclick={() => deleteProfile(p)} data-testid="profile-delete-{p}">
                  删除
                </button>
              </td>
            </tr>
            {#if digestFor === p}
              <tr>
                <td colspan="2">
                  {#if digest.length === 0}
                    <div class="empty">（空 profile）</div>
                  {:else}
                    <table class="digest" data-testid="profile-digest">
                      <thead>
                        <tr><th>entry</th><th>域</th><th>标志</th><th>值</th><th style="width:1%"></th></tr>
                      </thead>
                      <tbody>
                        {#each digest as e (e.key)}
                          <tr>
                            <td class="mono">{e.key}</td>
                            <td class="mono">{e.domain ?? "—"}</td>
                            <td>
                              {#if e.httpOnly}<span class="tag info">httpOnly</span>{/if}
                              {#if e.secure}<span class="tag info">secure</span>{/if}
                            </td>
                            <td class="mono muted">•••（{e.value_bytes} B）</td>
                            <td class="row-actions">
                              <button
                                class="danger"
                                onclick={() => deleteProfileEntry(p, e.key)}
                                data-testid="profile-entry-delete-{e.key}"
                              >
                                删除
                              </button>
                            </td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  {/if}
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    {/if}
  </div>
{/if}

<style>
  .settings-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: 14px;
  }

  .subject {
    font-size: 15px;
    margin-bottom: 10px;
  }

  .scope-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .form-col {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  /* profile 摘要的嵌套表格：与外层行区分开 */
  table.digest {
    margin: 6px 0;
  }
</style>
