<script lang="ts">
  import type { ConsoleApi, ProcInfo } from "../lib/api";
  import {
    parseSnapshotText,
    interactive,
    acceptsText,
    screencastInterval,
    takeoverView,
    type SnapshotElement,
    type TakeoverView,
  } from "../lib/session";

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

  const view = $derived<TakeoverView>(takeoverView(holder, self));
  const procState = $derived(procs.find((p) => p.pid === pid)?.state ?? null);

  async function refreshProcs() {
    try {
      procs = await api.procList();
      if (!pid && procs.length > 0) pid = procs[0].pid;
    } catch (e) {
      error = message(e);
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
      await Promise.all([captureFrame(), refreshSnapshot()]);
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
      captureFrame();
      refreshSnapshot();
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
  <div class="empty">无进程可查看。先在 Dashboard 里 Spawn 一个。</div>
{:else}
  <div class="split">
    <div class="card viewport">
      <h3>实时画面 <small class="muted">view.screenshot @2fps</small></h3>
      {#if frame}
        <img src={frame} alt="screencast frame" data-testid="screencast" />
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
