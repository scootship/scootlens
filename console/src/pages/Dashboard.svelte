<script lang="ts">
  import type { ConsoleApi, SysInfo, ProcInfo } from "../lib/api";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  let info = $state<SysInfo | null>(null);
  let procs = $state<ProcInfo[]>([]);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function load() {
    error = null;
    try {
      [info, procs] = await Promise.all([api.sysInfo(), api.procList()]);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function spawn() {
    busy = true;
    try {
      await api.procSpawn();
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }

  async function kill(pid: string) {
    try {
      await api.procKill(pid);
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  // 初次挂载 + 每次 pulse（evt 通知）时刷新。
  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <h2>Dashboard</h2>
  <button class="primary" onclick={spawn} disabled={busy} data-testid="spawn">
    {busy ? "启动中…" : "Spawn"}
  </button>
  <button onclick={load}>刷新</button>
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
      <div class="stat"><small>{info.abi_version}</small></div>
    </div>
    <div class="card">
      <h3>内核版本</h3>
      <div class="stat"><small>{info.kernel_version}</small></div>
    </div>
  </div>
{/if}

<div class="card">
  <h3>进程</h3>
  {#if procs.length === 0}
    <div class="empty">无运行中进程</div>
  {:else}
    <table>
      <thead>
        <tr><th>PID</th><th>状态</th><th>引擎</th><th>URL</th><th></th></tr>
      </thead>
      <tbody>
        {#each procs as p (p.pid)}
          <tr>
            <td class="mono">{p.pid}</td>
            <td>{p.state}</td>
            <td class="muted">{p.engine ?? "—"}</td>
            <td class="muted">{p.url ?? "—"}</td>
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
  {/if}
</div>
