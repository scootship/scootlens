<script lang="ts">
  import type { ConsoleApi, ProcInfo, NetLogEntry } from "../lib/api";
  import { formatTs } from "../lib/format";

  let {
    api,
    pulse,
    events,
  }: { api: ConsoleApi; pulse: number; events: { seq: number; topic: string; text: string }[] } =
    $props();

  let procs = $state<ProcInfo[]>([]);
  let pid = $state("");
  let snapshot = $state("");
  let netLog = $state<NetLogEntry[]>([]);
  let error = $state<string | null>(null);

  function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  async function refreshProcs() {
    try {
      procs = await api.procList();
      if (!pid && procs.length > 0) pid = procs[0].pid;
    } catch (e) {
      error = message(e);
    }
  }

  async function load() {
    if (!pid) return;
    error = null;
    const [snapRes, netRes] = await Promise.allSettled([
      api.snapshotText(pid),
      api.netLog(pid, 50),
    ]);
    if (snapRes.status === "fulfilled") snapshot = snapRes.value;
    else error = message(snapRes.reason);
    if (netRes.status === "fulfilled") netLog = netRes.value;
    else error = message(netRes.reason);
  }

  $effect(() => {
    void pulse;
    refreshProcs();
  });

  $effect(() => {
    if (pid) load();
  });
</script>

<div class="section-head">
  <h2>Inspector</h2>
  <select bind:value={pid} data-testid="inspector-pid">
    {#each procs as p (p.pid)}
      <option value={p.pid}>{p.pid} · {p.state}</option>
    {/each}
  </select>
  <button class="primary" onclick={load}>刷新</button>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}

{#if !pid}
  <div class="empty">无进程可检查。</div>
{:else}
  <div class="split">
    <div class="card">
      <h3>语义快照 <small class="muted">view.snapshot（Agent 视角的页面）</small></h3>
      {#if snapshot}
        <pre class="snapshot" data-testid="snapshot-text">{snapshot}</pre>
      {:else}
        <div class="empty">暂无快照</div>
      {/if}
    </div>

    <div>
      <div class="card">
        <h3>net.log <small class="muted">策略判定后的请求</small></h3>
        {#if netLog.length === 0}
          <div class="empty">暂无网络记录</div>
        {:else}
          <table>
            <thead><tr><th>时间</th><th>方法</th><th>URL</th><th>判定</th></tr></thead>
            <tbody>
              {#each netLog as e, i (i)}
                <tr>
                  <td class="muted">{typeof e.ts_ms === "number" ? formatTs(e.ts_ms) : "—"}</td>
                  <td class="mono">{e.method ?? "GET"}</td>
                  <td class="mono">{e.url ?? "?"}</td>
                  <td>
                    {#if e.allowed === false}
                      <span class="tag danger">blocked</span>
                    {:else}
                      <span class="tag ok">allow</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>

      <div class="card">
        <h3>事件流 <small class="muted">evt.subscribe（最近 {events.length} 条）</small></h3>
        {#if events.length === 0}
          <div class="empty">暂无事件</div>
        {:else}
          <table data-testid="event-stream">
            <thead><tr><th>seq</th><th>主题</th><th>载荷</th></tr></thead>
            <tbody>
              {#each events.slice(-30).reverse() as ev (ev.seq)}
                <tr>
                  <td class="mono">{ev.seq}</td>
                  <td><span class="tag info">{ev.topic}</span></td>
                  <td class="mono muted">{ev.text}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>
    </div>
  </div>
{/if}
