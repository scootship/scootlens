<script lang="ts">
  import type { ConsoleApi, SysInfo, ProcInfo } from "../lib/api";
  import { splitProcs, stateTone } from "../lib/procs";
  import { friendlyError } from "../lib/errors";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  let info = $state<SysInfo | null>(null);
  let procs = $state<ProcInfo[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** 是否展开已终止进程（默认收起：terminated 会持续累积，不该霸屏）。 */
  let showTerminated = $state(false);

  const view = $derived(splitProcs(procs));
  const rows = $derived(showTerminated ? [...view.active, ...view.terminated] : view.active);

  async function load() {
    error = null;
    try {
      [info, procs] = await Promise.all([api.sysInfo(), api.procList()]);
    } catch (e) {
      error = friendlyError(e);
    }
  }

  async function spawn() {
    busy = true;
    try {
      await api.procSpawn();
      await load();
    } catch (e) {
      error = friendlyError(e);
    } finally {
      busy = false;
    }
  }

  async function kill(pid: string) {
    try {
      await api.procKill(pid);
      await load();
    } catch (e) {
      error = friendlyError(e);
    }
  }

  // 初次挂载 + 每次 pulse（evt 通知）时刷新。
  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <span class="spacer"></span>
  <button class="ghost" onclick={load}>刷新</button>
  <button class="primary" onclick={spawn} disabled={busy} data-testid="spawn">
    {busy ? "启动中…" : "＋ Spawn"}
  </button>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}

{#if info}
  <div class="grid">
    <div class="card">
      <h3>引擎</h3>
      <div class="stat">{info.engine}</div>
    </div>
    <div class="card">
      <h3>进程配额</h3>
      <div class="stat">
        {info.running_procs}<small> / {info.max_procs}</small>
      </div>
    </div>
    <div class="card">
      <h3>ABI 版本</h3>
      <div class="stat">{info.abi_version}</div>
    </div>
    <div class="card">
      <h3>内核版本</h3>
      <div class="stat">{info.kernel_version}</div>
    </div>
  </div>
{/if}

<div class="card">
  <div class="proc-head">
    <h3 style="margin:0">
      进程
      <small class="muted">活跃 {view.active.length}</small>
    </h3>
    <span class="spacer"></span>
    {#if view.terminated.length > 0}
      <button class="link" onclick={() => (showTerminated = !showTerminated)} data-testid="toggle-terminated">
        {showTerminated
          ? "收起已终止"
          : `显示已终止（${view.terminated.length}）`}
      </button>
    {/if}
  </div>

  {#if rows.length === 0}
    <div class="empty">
      {#if view.terminated.length > 0}
        无活跃进程（{view.terminated.length} 个已终止进程被收起）
      {:else}
        无进程。点右上「Spawn」新建一个会话进程。
      {/if}
    </div>
  {:else}
    <div class="table-scroll">
      <table>
        <thead>
          <tr><th>PID</th><th>状态</th><th>引擎</th><th>URL</th><th></th></tr>
        </thead>
        <tbody>
          {#each rows as p (p.pid)}
            <tr class:dim={p.state === "terminated"}>
              <td class="mono">{p.pid}</td>
              <td><span class="tag {stateTone(p.state)}">{p.state}</span></td>
              <td class="muted">{p.engine ?? "—"}</td>
              <td class="muted mono">{p.url ?? "—"}</td>
              <td class="row-actions">
                {#if p.state !== "terminated"}
                  <button class="danger" onclick={() => kill(p.pid)} data-testid="kill-{p.pid}">
                    Kill
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

<style>
  .proc-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
  }
</style>
