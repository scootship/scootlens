<script lang="ts">
  import type { ConsoleApi } from "../lib/api";
  import { checkIntegrity, type JournalEntry, type IntegrityReport } from "../lib/journal";
  import { formatTs, kindLabel, kindTone } from "../lib/format";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

  let entries = $state<JournalEntry[]>([]);
  let report = $state<IntegrityReport | null>(null);
  let limit = $state(100);
  let pidFilter = $state("");
  let error = $state<string | null>(null);

  async function load() {
    error = null;
    try {
      const pid = pidFilter.trim() || undefined;
      entries = await api.journal(limit, pid);
      // 完整性仅对未过滤窗口有意义（pid 过滤会天然产生 seq 缺口）。
      report = pid ? null : checkIntegrity(entries);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <h2>Journal</h2>
  {#if report}
    {#if report.ok}
      <span class="tag ok">链完整 · {report.count} 条</span>
    {:else}
      <span class="tag danger">
        异常：{report.gaps.length} 缺口 / {report.missingHash.length} 缺 hash
      </span>
    {/if}
  {/if}
</div>

<div class="toolbar">
  <label class="check">
    limit
    <select bind:value={limit} onchange={load}>
      <option value={50}>50</option>
      <option value={100}>100</option>
      <option value={200}>200</option>
    </select>
  </label>
  <input placeholder="按 pid 过滤（可选）" bind:value={pidFilter} />
  <button class="primary" onclick={load}>刷新</button>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}

<div class="card">
  {#if entries.length === 0}
    <div class="empty">暂无审计条目</div>
  {:else}
    <table>
      <thead>
        <tr><th>seq</th><th>时间</th><th>类型</th><th>主体</th><th>方法</th><th>pid</th></tr>
      </thead>
      <tbody>
        {#each entries as e (e.seq)}
          <tr>
            <td class="mono">{e.seq}</td>
            <td class="muted">{formatTs(e.ts_ms)}</td>
            <td><span class="tag {kindTone(e.kind)}">{kindLabel(e.kind)}</span></td>
            <td class="mono">{e.subject}</td>
            <td class="mono">{e.method}</td>
            <td class="muted">{e.pid ?? "—"}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>
