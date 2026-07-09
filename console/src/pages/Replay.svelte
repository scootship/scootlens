<script lang="ts">
  import type { ConsoleApi, ProcInfo } from "../lib/api";
  import {
    parseBundle,
    verifyChain,
    timeline,
    frameAt,
    frameUrl,
    type ReplayBundle,
    type ChainReport,
    type TimelineItem,
  } from "../lib/replay";
  import { formatTs, kindLabel, kindTone } from "../lib/format";
  import { sortProcs, preferredPid } from "../lib/procs";
  import { friendlyError } from "../lib/errors";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  let procs = $state<ProcInfo[]>([]);
  let pid = $state("");
  let bundle = $state<ReplayBundle | null>(null);
  let report = $state<ChainReport | null>(null);
  let items = $state<TimelineItem[]>([]);
  let cursor = $state(0);
  let onlyPid = $state(true);
  let error = $state<string | null>(null);

  const visible = $derived(onlyPid ? items.filter((i) => i.ofPid) : items);
  const current = $derived(visible.length > 0 ? visible[Math.min(cursor, visible.length - 1)] : null);
  const currentFrame = $derived(
    bundle && current ? frameAt(bundle.frames, current.entry.ts_ms) : null,
  );

  function message(e: unknown): string {
    return friendlyError(e);
  }

  async function loadBundle(raw: unknown) {
    try {
      const b = parseBundle(raw);
      bundle = b;
      items = timeline(b);
      report = await verifyChain(b.journal);
      cursor = 0;
      error = null;
    } catch (e) {
      error = message(e);
      bundle = null;
      report = null;
      items = [];
    }
  }

  async function exportLive() {
    if (!pid) return;
    try {
      await loadBundle(await api.replayExport(pid));
    } catch (e) {
      error = message(e);
    }
  }

  async function openFile(ev: Event) {
    const input = ev.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    await loadBundle(await file.text());
    input.value = "";
  }

  function download() {
    if (!bundle) return;
    const blob = new Blob([JSON.stringify(bundle, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `replay-${bundle.pid}-${bundle.exported_at_ms}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  }

  $effect(() => {
    void pulse;
    api
      .procList()
      .then((p) => {
        procs = sortProcs(p);
        if (!pid && p.length > 0) pid = preferredPid(p);
      })
      .catch((e) => (error = message(e)));
  });
</script>

<div class="section-head">
  <select bind:value={pid} data-testid="replay-pid">
    {#each procs as p (p.pid)}
      <option value={p.pid}>{p.pid} · {p.state}</option>
    {/each}
  </select>
  <button class="primary" onclick={exportLive} disabled={!pid} data-testid="replay-export">
    导出回放包
  </button>
  <label class="check">
    离线打开 <input type="file" accept="application/json" onchange={openFile} />
  </label>
  {#if bundle}
    <button onclick={download}>下载 .json</button>
  {/if}
  <span class="spacer"></span>
  {#if report}
    {#if report.ok}
      <span class="tag ok" data-testid="chain-status">哈希链完整 · {report.checked} 行</span>
    {:else}
      <span class="tag danger" data-testid="chain-status">
        链校验失败 @seq {report.brokenAt}：{report.reason}
      </span>
    {/if}
  {/if}
</div>

{#if error}
  <div class="error">{error}</div>
{/if}

{#if !bundle}
  <div class="empty">导出运行中进程的回放包，或离线打开一个 replay-*.json。</div>
{:else}
  <div class="split">
    <div class="card viewport">
      <h3>
        画面 <small class="muted">{bundle.frames.length} 帧 · {bundle.engine} · {bundle.pid}</small>
      </h3>
      {#if currentFrame}
        <img src={frameUrl(currentFrame)} alt="replay frame" data-testid="replay-frame" />
      {:else}
        <div class="empty">当前时间点之前无帧</div>
      {/if}
      <div class="toolbar">
        <button onclick={() => (cursor = Math.max(0, cursor - 1))} disabled={cursor === 0}>
          ← 上一步
        </button>
        <input
          type="range"
          min="0"
          max={Math.max(0, visible.length - 1)}
          bind:value={cursor}
          style="flex:1"
          data-testid="replay-cursor"
        />
        <button
          onclick={() => (cursor = Math.min(visible.length - 1, cursor + 1))}
          disabled={cursor >= visible.length - 1}
        >
          下一步 →
        </button>
        <label class="check"><input type="checkbox" bind:checked={onlyPid} /> 仅本 pid</label>
      </div>
      {#if current}
        <div class="muted">
          步骤 {cursor + 1}/{visible.length} · {formatTs(current.entry.ts_ms)} ·
          <span class="mono">{current.entry.method}</span>
        </div>
      {/if}
    </div>

    <div class="card">
      <h3>syscall 时间线 <small class="muted">journal 链段（旧→新）</small></h3>
      {#if visible.length === 0}
        <div class="empty">该 pid 在链段内无记录</div>
      {:else}
        <table data-testid="replay-timeline">
          <thead><tr><th>seq</th><th>时间</th><th>类型</th><th>方法</th><th>主体</th></tr></thead>
          <tbody>
            {#each visible as item, i (item.entry.seq)}
              <tr
                class:active={i === cursor}
                onclick={() => (cursor = i)}
                style="cursor:pointer"
              >
                <td class="mono">{item.entry.seq}</td>
                <td class="muted">{formatTs(item.entry.ts_ms)}</td>
                <td><span class="tag {kindTone(item.entry.kind)}">{kindLabel(item.entry.kind)}</span></td>
                <td class="mono">{item.entry.method}</td>
                <td class="mono muted">{item.entry.subject}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>
{/if}
