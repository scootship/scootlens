<script lang="ts">
  import type { ConsoleApi, ProcInfo } from "../lib/api";
  import {
    parseSnapshotText,
    interactive,
    acceptsText,
    screencastInterval,
    takeoverView,
    containRect,
    clickRatio,
    pickLoginFields,
    type SnapshotElement,
    type TakeoverView,
  } from "../lib/session";
  import { listProfiles, rememberProfile } from "../lib/profiles";
  import {
    listCredentials,
    matchingCredentials,
    originMatches,
    type CredentialProfile,
  } from "../lib/credentials";

  let { api, pulse, self }: { api: ConsoleApi; pulse: number; self: string } = $props();

  let procs = $state<ProcInfo[]>([]);
  let pid = $state("");
  let frame = $state<string | null>(null);
  let elements = $state<SnapshotElement[]>([]);
  let holder = $state<string | null>(null);
  let typeText = $state("");
  let gotoUrl = $state("");
  let error = $state<string | null>(null);
  let live = $state(true);
  const NEW_PROFILE = "__new__";
  let knownProfiles = $state<string[]>(listProfiles());
  let profileChoice = $state<string>(listProfiles()[0] ?? "");
  let newProfile = $state("");
  let spawning = $state(false);
  let credentials = $state<CredentialProfile[]>(listCredentials());
  let credentialChoice = $state("");

  const view = $derived<TakeoverView>(takeoverView(holder, self));
  const proc = $derived(procs.find((p) => p.pid === pid) ?? null);
  const procState = $derived(proc?.state ?? null);
  const matchedCredentials = $derived(matchingCredentials(proc?.url, credentials));
  const selectedCredential = $derived(
    matchedCredentials.find((c) => c.id === credentialChoice) ?? matchedCredentials[0] ?? null,
  );
  // 实际用于 spawn 的 profile 名：新建时取输入框，否则取下拉选中项（空=默认空白会话）。
  const effProfile = $derived(profileChoice === NEW_PROFILE ? newProfile.trim() : profileChoice.trim());

  async function refreshProcs() {
    try {
      procs = await api.procList();
      if (!pid && procs.length > 0) pid = procs[0].pid;
    } catch (e) {
      error = message(e);
    }
  }

  /** 用选中的 profile 新开会话：spawn 时预加载该 profile 的登录态（cookie），
   *  随后即可导航 + 接管，带登录态操作。profile 留空则默认空白会话。
   *  成功后把 profile 名记入本地下拉，方便下次直接选。 */
  async function spawnWithProfile() {
    spawning = true;
    try {
      const prof = effProfile;
      const newPid = await api.procSpawn(prof || undefined);
      if (prof) {
        knownProfiles = rememberProfile(prof);
        profileChoice = prof;
        newProfile = "";
      }
      await refreshProcs();
      pid = newPid;
      error = null;
    } catch (e) {
      error = message(e);
    } finally {
      spawning = false;
    }
  }

  function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function captureFrame() {
    if (!pid) return;
    try {
      frame = await api.screenshot(pid);
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function refreshSnapshot() {
    if (!pid) return;
    try {
      elements = interactive(parseSnapshotText(await api.snapshotText(pid)));
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function takeover() {
    try {
      await api.takeoverStart(pid);
      holder = self;
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function release() {
    try {
      await api.takeoverEnd(pid);
      holder = null;
      error = null;
    } catch (e) {
      error = message(e);
    }
  }

  async function inject(kind: "click" | "type", el: SnapshotElement) {
    if (!el.ref) return;
    try {
      if (kind === "click") await api.actClick(pid, el.ref);
      else await api.actType(pid, el.ref, typeText);
      error = null;
      await Promise.all([captureFrame(), refreshSnapshot()]);
    } catch (e) {
      error = message(e);
    }
  }

  async function fillCredential(c: CredentialProfile | null) {
    if (!c) return;
    if (!proc?.url || !originMatches(c.origin, proc.url)) {
      error = "当前页面 origin 与凭据绑定不匹配";
      return;
    }
    const fields = pickLoginFields(elements);
    if (!fields.username?.ref || !fields.password?.ref) {
      error = "未找到可填充的用户名/密码输入框，请刷新元素或手动选择字段";
      return;
    }
    try {
      await api.actTypeVault(pid, fields.username.ref, c.usernameRef);
      await api.actTypeVault(pid, fields.password.ref, c.passwordRef);
      error = null;
      await Promise.all([captureFrame(), refreshSnapshot(), refreshProcs()]);
    } catch (e) {
      error = message(e);
    }
  }

  /** 直接点击画面（仅接管中生效）：把点击偏移换算成归一化坐标 → act.point.click。 */
  async function pointClick(ev: MouseEvent) {
    if (view.kind !== "held-by-me") return;
    const img = ev.currentTarget as HTMLImageElement;
    const box = containRect(img.naturalWidth, img.naturalHeight, img.clientWidth, img.clientHeight);
    const ratio = box ? clickRatio(ev.offsetX, ev.offsetY, box) : null;
    if (!ratio) return;
    try {
      await api.actClickAt(pid, ratio.xRatio, ratio.yRatio);
      error = null;
      await Promise.all([captureFrame(), refreshSnapshot()]);
    } catch (e) {
      error = message(e);
    }
  }

  async function pressEnter() {
    try {
      await api.actPress(pid, "Enter");
      await Promise.all([captureFrame(), refreshSnapshot()]);
    } catch (e) {
      error = message(e);
    }
  }

  async function goto() {
    if (!gotoUrl.trim()) return;
    try {
      await api.navGoto(pid, gotoUrl.trim());
      await Promise.all([captureFrame(), refreshSnapshot(), refreshProcs()]);
    } catch (e) {
      error = message(e);
    }
  }

  // pulse（evt 通知）→ 进程列表刷新
  $effect(() => {
    void pulse;
    refreshProcs();
  });

  // pid 变化 → 立即取一帧 + 快照
  $effect(() => {
    if (pid) {
      frame = null;
      holder = null;
      credentials = listCredentials();
      captureFrame();
      refreshSnapshot();
    }
  });

  $effect(() => {
    if (matchedCredentials.length === 0) {
      credentialChoice = "";
    } else if (!matchedCredentials.some((c) => c.id === credentialChoice)) {
      credentialChoice = matchedCredentials[0].id;
    }
  });

  // screencast 轮询（running + live 开启时）
  $effect(() => {
    const ms = live ? screencastInterval(procState) : 0;
    if (!ms || !pid) return;
    const t = setInterval(captureFrame, ms);
    return () => clearInterval(t);
  });
</script>

<div class="section-head">
  <h2>Session</h2>
  <select class="profile-in" bind:value={profileChoice} data-testid="spawn-profile" title="选择要复用登录态的 profile">
    <option value="">（默认·空白）</option>
    {#each knownProfiles as p (p)}
      <option value={p}>{p}</option>
    {/each}
    <option value={NEW_PROFILE}>＋ 新建…</option>
  </select>
  {#if profileChoice === NEW_PROFILE}
    <input
      class="profile-in"
      placeholder="新 profile 名"
      bind:value={newProfile}
      data-testid="spawn-profile-new"
    />
  {/if}
  <button
    class="primary"
    onclick={spawnWithProfile}
    disabled={spawning || (profileChoice === NEW_PROFILE && !newProfile.trim())}
    data-testid="spawn-with-profile"
  >
    {spawning ? "启动中…" : "新开会话"}
  </button>
  <select bind:value={pid} data-testid="session-pid">
    {#each procs as p (p.pid)}
      <option value={p.pid}>{p.pid} · {p.state}</option>
    {/each}
  </select>
  <label class="check"><input type="checkbox" bind:checked={live} /> 实时</label>
  <span class="spacer"></span>
  {#if view.kind === "held-by-me"}
    <span class="tag warn">接管中 · {self}</span>
    <button class="primary" onclick={release} data-testid="release">归还控制</button>
  {:else if view.kind === "held-by-other"}
    <span class="tag danger">被 {view.holder} 接管</span>
  {:else}
    <button class="primary" disabled={!pid || procState !== "running"} onclick={takeover} data-testid="takeover">
      接管
    </button>
  {/if}
</div>

{#if error}
  <div class="error">{error}</div>
{/if}

{#if !pid}
  <div class="empty">无进程可查看。上方选 profile 点「新开会话」，或在 Dashboard 里 Spawn。</div>
{:else}
  <div class="split">
    <div class="card viewport">
      <h3>
        实时画面 <small class="muted">view.screenshot @2fps</small>
        {#if view.kind === "held-by-me"}
          <small class="muted">· 接管中，可直接点击画面</small>
        {/if}
      </h3>
      {#if frame}
        <!--
          画面点击是坐标/指针交互，没有有意义的键盘等价物（一次按键无法表达
          "点在这个像素"）；键盘可达的等价路径始终保留在下方"输入注入"面板
          （语义元素清单 → Click/Type 按钮）。
        -->
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
          src={frame}
          alt="screencast frame"
          data-testid="screencast"
          class:interactive={view.kind === "held-by-me"}
          onclick={pointClick}
        />
      {:else}
        <div class="empty">等待帧…（挂起/终止的进程无画面）</div>
      {/if}
      <div class="toolbar">
        <input placeholder="https://…" bind:value={gotoUrl} data-testid="goto-url" />
        <button onclick={goto} disabled={view.kind === "held-by-other"}>导航</button>
        <button onclick={pressEnter} disabled={view.kind === "held-by-other"}>⏎ Enter</button>
      </div>
    </div>

    <div class="card">
      <h3>输入注入 <small class="muted">语义元素 → act.*</small></h3>
      {#if matchedCredentials.length}
        <div class="toolbar">
          <select
            bind:value={credentialChoice}
            data-testid="credential-choice"
            title="当前页面 origin 命中的凭据绑定"
          >
            {#each matchedCredentials as c (c.id)}
              <option value={c.id}>{c.label} · {c.origin}</option>
            {/each}
          </select>
          <button
            class="primary"
            onclick={() => fillCredential(selectedCredential)}
            disabled={view.kind === "held-by-other"}
            data-testid="credential-fill"
          >
            填入凭据
          </button>
          {#if selectedCredential?.loginUrl}
            <button
              onclick={() => {
                gotoUrl = selectedCredential.loginUrl ?? "";
                goto();
              }}
              disabled={view.kind === "held-by-other"}
              data-testid="credential-login-url"
            >
              打开登录页
            </button>
          {/if}
        </div>
      {/if}
      <div class="toolbar">
        <input placeholder="Type 文本…" bind:value={typeText} data-testid="type-text" />
        <button onclick={refreshSnapshot}>刷新元素</button>
      </div>
      {#if elements.length === 0}
        <div class="empty">无可交互元素</div>
      {:else}
        <table>
          <thead><tr><th>元素</th><th>ref</th><th></th></tr></thead>
          <tbody>
            {#each elements as el (el.ref)}
              <tr>
                <td style="padding-left: {el.depth * 12}px">
                  <span class="tag info">{el.role}</span> {el.name}
                  {#if el.value}<small class="muted">= {el.value}</small>{/if}
                </td>
                <td class="mono">{el.ref}</td>
                <td class="row-actions">
                  <button onclick={() => inject("click", el)} disabled={view.kind === "held-by-other"}>
                    Click
                  </button>
                  {#if acceptsText(el.role)}
                    <button onclick={() => inject("type", el)} disabled={view.kind === "held-by-other"}>
                      Type
                    </button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
{/if}
