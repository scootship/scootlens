<script lang="ts">
  import { untrack } from "svelte";
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
  import { sortProcs, preferredActivePid, selectableProcs, stateTone } from "../lib/procs";
  import { pressKeyFor } from "../lib/keys";
  import { friendlyError } from "../lib/errors";

  let { api, pulse, self }: { api: ConsoleApi; pulse: number; self: string } = $props();

  let procs = $state<ProcInfo[]>([]);
  let pid = $state("");
  let frame = $state<string | null>(null);
  let elements = $state<SnapshotElement[]>([]);
  let holder = $state<string | null>(null);
  let typeText = $state("");
  let gotoUrl = $state("");
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let live = $state(true);
  /** 画面模态放大。 */
  let expanded = $state(false);
  /** 保存登录态目标 profile 名（默认沿用 spawn 选择）。 */
  let saveProfile = $state("");
  let savingProfile = $state(false);
  let killing = $state(false);
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
  const options = $derived(selectableProcs(procs, pid));
  const matchedCredentials = $derived(matchingCredentials(proc?.url, credentials));
  const selectedCredential = $derived(
    matchedCredentials.find((c) => c.id === credentialChoice) ?? matchedCredentials[0] ?? null,
  );
  // 实际用于 spawn 的 profile 名：新建时取输入框，否则取下拉选中项（空=默认空白会话）。
  const effProfile = $derived(profileChoice === NEW_PROFILE ? newProfile.trim() : profileChoice.trim());

  async function refreshProcs() {
    try {
      procs = sortProcs(await api.procList());
      // 无选中，或选中的进程已消失/终止且存在活跃进程 → 自动切到首个活跃进程
      const cur = procs.find((p) => p.pid === pid);
      if (!pid || !cur) pid = preferredActivePid(procs) || pid;
    } catch (e) {
      error = friendlyError(e);
    }
  }

  /** 用选中的 profile 新开会话：spawn 时预加载该 profile 的登录态（cookie），
   *  随后即可导航 + 接管，带登录态操作。profile 留空则默认空白会话。 */
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
      notice = prof ? `已用 profile「${prof}」新开会话 ${newPid}` : `已新开会话 ${newPid}`;
    } catch (e) {
      error = friendlyError(e);
    } finally {
      spawning = false;
    }
  }

  /** 快速终止当前会话，并自动切到下一个活跃进程。 */
  async function killCurrent() {
    if (!pid) return;
    killing = true;
    try {
      await api.procKill(pid);
      const dead = pid;
      holder = null;
      expanded = false;
      await refreshProcs();
      const next = preferredActivePid(procs.filter((p) => p.pid !== dead));
      pid = next;
      notice = `已终止 ${dead}`;
      error = null;
    } catch (e) {
      error = friendlyError(e);
    } finally {
      killing = false;
    }
  }

  /** 把当前会话的登录态（cookies + storage）保存为 profile：
   *  state.export → state.import。之后「新开会话」选该 profile 即复用登录态。 */
  async function saveAsProfile() {
    const prof = saveProfile.trim();
    if (!pid || !prof) return;
    savingProfile = true;
    try {
      const bundle = await api.stateExport(pid);
      await api.stateImport(prof, bundle);
      knownProfiles = rememberProfile(prof);
      profileChoice = prof;
      notice = `已把 ${pid} 的登录态保存为 profile「${prof}」，新开会话选它即可复用`;
      error = null;
    } catch (e) {
      error = friendlyError(e);
    } finally {
      savingProfile = false;
    }
  }

  async function captureFrame() {
    if (!pid || procState === "terminated") return;
    try {
      frame = await api.screenshot(pid);
      error = null;
    } catch (e) {
      error = friendlyError(e);
    }
  }

  async function refreshSnapshot() {
    if (!pid || procState === "terminated") return;
    try {
      elements = interactive(parseSnapshotText(await api.snapshotText(pid)));
      error = null;
    } catch (e) {
      error = friendlyError(e);
    }
  }

  async function takeover() {
    try {
      await api.takeoverStart(pid);
      holder = self;
      error = null;
    } catch (e) {
      error = friendlyError(e, "act.takeover.start");
    }
  }

  async function release() {
    try {
      await api.takeoverEnd(pid);
      holder = null;
      error = null;
    } catch (e) {
      error = friendlyError(e, "act.takeover.end");
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
      error = friendlyError(e);
    }
  }

  /** 凭据绑定填充：vault_ref 经内核解引用注入，明文不经过 Console。 */
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
      error = friendlyError(e, "act.type");
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
      error = friendlyError(e, "act.point.click");
    }
  }

  /** 接管中的键盘透传：可打印字符与常用控制键 → act.press；
   *  组合键（Cmd/Ctrl/Alt）留给用户浏览器。输入焦点在表单控件时不拦截。 */
  async function handleKey(ev: KeyboardEvent) {
    if (view.kind !== "held-by-me") return;
    const t = ev.target as HTMLElement | null;
    if (t && ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName)) return;
    const key = pressKeyFor(ev);
    if (!key) return;
    ev.preventDefault();
    try {
      await api.actPress(pid, key);
      error = null;
      captureFrame();
    } catch (e) {
      error = friendlyError(e, "act.press");
    }
  }

  async function goto() {
    if (!gotoUrl.trim()) return;
    try {
      await api.navGoto(pid, gotoUrl.trim());
      // refreshProcs 同步 proc.url（凭据绑定按当前 origin 匹配）
      await Promise.all([captureFrame(), refreshSnapshot(), refreshProcs()]);
    } catch (e) {
      error = friendlyError(e);
    }
  }

  // pulse（evt 通知）→ 进程列表刷新
  $effect(() => {
    void pulse;
    refreshProcs();
  });

  // pid 变化 → 立即取一帧 + 快照（untrack：内部读 procState，不让 procs
  // 刷新反复触发本 effect 重置画面/接管状态）
  $effect(() => {
    if (pid) {
      frame = null;
      holder = null;
      credentials = listCredentials();
      untrack(() => {
        captureFrame();
        refreshSnapshot();
      });
    }
  });

  // 命中凭据变化时校正下拉选中项
  $effect(() => {
    if (matchedCredentials.length === 0) {
      credentialChoice = "";
    } else if (!matchedCredentials.some((c) => c.id === credentialChoice)) {
      credentialChoice = matchedCredentials[0].id;
    }
  });

  // screencast 轮询（running + live 开启时；模态打开时提速）
  $effect(() => {
    const base = live ? screencastInterval(procState) : 0;
    const ms = expanded && base ? Math.max(250, base / 2) : base;
    if (!ms || !pid) return;
    const t = setInterval(captureFrame, ms);
    return () => clearInterval(t);
  });

  // 模态打开时监听全局键盘（透传 + Escape 关闭）
  $effect(() => {
    if (!expanded) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape" && view.kind !== "held-by-me") {
        expanded = false;
        return;
      }
      handleKey(ev);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });
</script>

<div class="section-head">
  {#if proc}
    <span class="tag {stateTone(proc.state)}">{pid} · {proc.state}</span>
  {/if}
  <span class="spacer"></span>
  {#if pid && procState !== "terminated"}
    <button class="danger" onclick={killCurrent} disabled={killing} data-testid="session-kill">
      {killing ? "终止中…" : "Kill 会话"}
    </button>
  {/if}
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

<div class="toolbar">
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
    {#each options as p (p.pid)}
      <option value={p.pid}>{p.pid} · {p.state}</option>
    {/each}
  </select>
  <label class="check"><input type="checkbox" bind:checked={live} /> 实时</label>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}
{#if notice}
  <div class="notice" data-testid="session-notice">{notice}</div>
{/if}

{#if !pid}
  <div class="empty">
    无活跃会话。上方选 profile 点「新开会话」，或在 Dashboard 里 Spawn。
  </div>
{:else if procState === "terminated"}
  <div class="empty" data-testid="terminated-panel">
    会话 {pid} 已终止，无实时画面（历史见 Replay / Journal）。<br />
    <button class="primary" style="margin-top:12px" onclick={spawnWithProfile} disabled={spawning}>
      {spawning ? "启动中…" : "新开会话"}
    </button>
  </div>
{:else}
  <div class="split">
    <div class="card viewport">
      <h3>
        实时画面 <small class="muted">view.screenshot</small>
        {#if view.kind === "held-by-me"}
          <small class="muted">· 接管中：点击画面 / 直接键盘输入</small>
        {/if}
        <span class="spacer"></span>
        <button class="ghost" onclick={() => (expanded = true)} disabled={!frame} data-testid="expand-view">
          ⛶ 放大
        </button>
      </h3>
      {#if frame}
        <!--
          画面点击是坐标/指针交互，没有有意义的键盘等价物；键盘可达的等价路径
          保留在右侧"输入注入"面板（语义元素清单 → Click/Type 按钮）。
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
        <div class="empty">等待帧…（挂起的进程无画面）</div>
      {/if}
      <div class="toolbar">
        <form style="display:flex; gap:10px; flex:1" onsubmit={(e) => { e.preventDefault(); goto(); }}>
          <input placeholder="https://…" bind:value={gotoUrl} data-testid="goto-url" style="flex:1; min-width:180px" />
          <button type="submit" disabled={view.kind === "held-by-other"}>导航</button>
        </form>
      </div>
      {#if view.kind === "held-by-me"}
        <p class="hint" style="margin:0">
          键盘输入直接透传到页面（Enter/Tab/方向键/字符）；Cmd/Ctrl 组合键留给本地浏览器。
        </p>
      {/if}
    </div>

    <div style="display:flex; flex-direction:column; gap:14px;">
      <div class="card">
        <h3>保存登录态 <small class="muted">state.export → profile（新会话复用）</small></h3>
        <p class="card-desc">
          接管登录完成后，把当前会话的 cookie/storage 存为 profile；之后「新开会话」
          从下拉选它即带登录态。
        </p>
        <div class="toolbar" style="margin:0">
          <input
            placeholder="profile 名，如 google"
            bind:value={saveProfile}
            list="known-profiles-session"
            data-testid="save-profile-name"
            style="flex:1; min-width:140px"
          />
          <datalist id="known-profiles-session">
            {#each knownProfiles as p (p)}
              <option value={p}></option>
            {/each}
          </datalist>
          <button
            class="primary"
            onclick={saveAsProfile}
            disabled={savingProfile || !saveProfile.trim() || procState !== "running"}
            data-testid="save-profile"
          >
            {savingProfile ? "保存中…" : "保存"}
          </button>
        </div>
      </div>

      <div class="card">
        <h3>输入注入 <small class="muted">语义元素 → act.*</small></h3>
        {#if matchedCredentials.length}
          <div class="toolbar credential-fill">
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
          <input placeholder="Type 文本…" bind:value={typeText} data-testid="type-text" style="flex:1; min-width:160px" />
          <button onclick={refreshSnapshot}>刷新元素</button>
        </div>
        {#if elements.length === 0}
          <div class="empty">无可交互元素</div>
        {:else}
          <div class="table-scroll elements-scroll">
            <table>
              <thead><tr><th>元素</th><th>ref</th><th></th></tr></thead>
              <tbody>
                {#each elements as el (el.ref)}
                  <tr>
                    <td style="padding-left: {10 + el.depth * 12}px">
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
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

{#if expanded && pid}
  <div class="view-modal" role="dialog" aria-label="实时画面放大">
    <button class="view-modal-backdrop" aria-label="关闭" onclick={() => (expanded = false)}></button>
    <div class="view-modal-body">
      <div class="view-modal-head">
        <span class="tag {stateTone(procState ?? '')}">{pid} · {procState}</span>
        {#if view.kind === "held-by-me"}
          <span class="tag warn">接管中 · 点击画面 / 直接键盘输入</span>
          <button class="primary" onclick={release}>归还控制</button>
        {:else if view.kind === "held-by-other"}
          <span class="tag danger">被 {view.holder} 接管</span>
        {:else}
          <button class="primary" disabled={procState !== "running"} onclick={takeover}>接管</button>
        {/if}
        <form style="display:flex; gap:8px; flex:1" onsubmit={(e) => { e.preventDefault(); goto(); }}>
          <input placeholder="https://…" bind:value={gotoUrl} style="flex:1" />
          <button type="submit" class="ghost" disabled={view.kind === "held-by-other"}>导航</button>
        </form>
        <button class="ghost" onclick={() => (expanded = false)} data-testid="close-modal">✕ 关闭</button>
      </div>
      {#if frame}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <img
          class="view-modal-frame"
          class:interactive={view.kind === "held-by-me"}
          src={frame}
          alt="screencast frame (enlarged)"
          data-testid="screencast-modal"
          onclick={pointClick}
        />
      {:else}
        <div class="empty">等待帧…</div>
      {/if}
    </div>
  </div>
{/if}
