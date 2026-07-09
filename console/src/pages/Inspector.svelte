<script lang="ts">
  import type { ConsoleApi, ProcInfo, NetLogEntry } from "../lib/api";
  import { formatTs } from "../lib/format";
  import { sortProcs, preferredPid, stateTone } from "../lib/procs";
  import { topicTone, summarizeEvent, type ConsoleEvent } from "../lib/events";
  import { friendlyError } from "../lib/errors";

  let {
    api,
    pulse,
    events,
  }: { api: ConsoleApi; pulse: number; events: ConsoleEvent[] } = $props();

  let procs = $state<ProcInfo[]>([]);
  let pid = $state("");
  let snapshot = $state("");
  let netLog = $state<NetLogEntry[]>([]);
  /** 分面板错误：终止进程取不到快照/网络日志属常态，不该顶部横幅报警。 */
  let snapError = $state<string | null>(null);
  let netError = $state<string | null>(null);
  let error = $state<string | null>(null);
  /** 事件流是否只看当前 pid。 */
  let onlyPid = $state(false);

  const proc = $derived(procs.find((p) => p.pid === pid) ?? null);
  const shown = $derived.by(() => {
    const list = onlyPid && pid
      ? events.filter((ev) => summarizeEvent(ev.text).pid === pid)
      : events;
    return list.slice(-60).reverse();
  });

  async function refreshProcs() {
    try {
      procs = sortProcs(await api.procList());
      if (!pid && procs.length > 0) pid = preferredPid(procs);
    } catch (e) {
      error = friendlyError(e);
    }
  }

  async function load() {
    if (!pid) return;
    error = null;
    const [snapRes, netRes] = await Promise.allSettled([
      api.snapshotText(pid),
      api.netLog(pid, 50),
    ]);
    if (snapRes.status === "fulfilled") {
      snapshot = snapRes.value;
      snapError = null;
    } else {
      snapshot = "";
      snapError = friendlyError(snapRes.reason);
    }
    if (netRes.status === "fulfilled") {
      netLog = netRes.value;
      netError = null;
    } else {
      netLog = [];
      netError = friendlyError(netRes.reason);
    }
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
  <select bind:value={pid} data-testid="inspector-pid">
    {#each procs as p (p.pid)}
      <option value={p.pid}>{p.pid} · {p.state}</option>
    {/each}
  </select>
  {#if proc}
    <span class="tag {stateTone(proc.state)}">{proc.state}</span>
  {/if}
  <span class="spacer"></span>
  <button class="ghost" onclick={load}>刷新</button>
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
      {:else if proc?.state === "terminated"}
        <div class="empty">进程已终止，无实时快照（历史见 Replay）</div>
      {:else if snapError}
        <div class="empty">快照不可用 · <span class="muted">{snapError}</span></div>
      {:else}
        <div class="empty">暂无快照</div>
      {/if}
    </div>

    <div style="display:flex; flex-direction:column; gap:14px;">
      <div class="card">
        <h3>net.log <small class="muted">策略判定后的请求</small></h3>
        {#if netLog.length === 0}
          {#if proc?.state === "terminated"}
            <div class="empty">进程已终止，无网络记录</div>
          {:else if netError}
            <div class="empty">网络日志不可用 · <span class="muted">{netError}</span></div>
          {:else}
            <div class="empty">暂无网络记录</div>
          {/if}
        {:else}
          <div class="table-scroll">
            <table>
              <thead><tr><th>时间</th><th>方法</th><th>URL</th><th>判定</th></tr></thead>
              <tbody>
                {#each netLog as e, i (i)}
                  <tr>
                    <td class="muted">{typeof e.ts_ms === "number" ? formatTs(e.ts_ms) : "—"}</td>
                    <td class="mono">{e.method ?? "GET"}</td>
                    <td class="mono" style="overflow-wrap:anywhere">{e.url ?? "?"}</td>
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
          </div>
        {/if}
      </div>

      <div class="card">
        <h3>
          事件流 <small class="muted">evt.subscribe · 最近 {shown.length} 条（新→旧）</small>
        </h3>
        <div class="toolbar">
          <label class="check">
            <input type="checkbox" bind:checked={onlyPid} /> 仅当前 pid
          </label>
        </div>
        {#if shown.length === 0}
          <div class="empty">暂无事件</div>
        {:else}
          <div class="event-list" data-testid="event-stream">
            {#each shown as ev (ev.seq)}
              {@const sum = summarizeEvent(ev.text)}
              <div class="event-row" title={ev.text}>
                <span class="seq">{ev.seq}</span>
                <span class="topic"><span class="tag {topicTone(ev.topic)}">{ev.topic}</span></span>
                {#if sum.pid}
                  <span class="ev-pid">{sum.pid}</span>
                {/if}
                <span class="fields">{sum.fields.join("  ")}</span>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}
