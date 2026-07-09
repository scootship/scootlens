<script lang="ts">
  import type { ConsoleApi, PendingApproval, ApprovalDecision } from "../lib/api";
  import { scopeLabel, formatTs, shortId } from "../lib/format";
  import { AUTO_APPROVE_RULES } from "../lib/autoapprove";
  import { friendlyError } from "../lib/errors";

  let {
    api,
    pulse,
    enabled,
    note,
    onToggle,
  }: {
    api: ConsoleApi;
    pulse: number;
    /** 已勾选的自动审批规则 id。 */
    enabled: ReadonlySet<string>;
    /** 最近一次自动批准说明（App 层写入）。 */
    note: string | null;
    onToggle: (id: string, on: boolean) => void;
  } = $props();

  let items = $state<PendingApproval[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<string | null>(null);

  async function load() {
    error = null;
    try {
      items = await api.pending();
    } catch (e) {
      error = friendlyError(e);
    }
  }

  async function decide(id: string, decision: ApprovalDecision, remember: boolean) {
    busy = id;
    error = null;
    try {
      await api.approve(id, decision, remember);
      await load();
    } catch (e) {
      error = friendlyError(e);
    } finally {
      busy = null;
    }
  }

  $effect(() => {
    void pulse;
    load();
  });
</script>

<div class="section-head">
  <span class="pill"><span class="led"></span>{items.length} 待审</span>
  <span class="spacer"></span>
  <button class="ghost" onclick={load}>刷新</button>
</div>

{#if error}
  <div class="error">{error}</div>
{/if}
{#if note}
  <div class="notice" data-testid="autoapprove-note">{note}</div>
{/if}

<div class="split">
  <div>
    {#if items.length === 0}
      <div class="empty">审批收件箱为空</div>
    {:else}
      {#each items as it (it.id)}
        <div class="approval">
          <div class="row"><span class="k">主体</span><code>{it.subject}</code></div>
          <div class="row"><span class="k">方法</span><code>{it.method}</code></div>
          <div class="row"><span class="k">作用域</span><code>{scopeLabel(it.scope)}</code></div>
          {#if it.reason}
            <div class="row"><span class="k">理由</span><span>{it.reason}</span></div>
          {/if}
          <div class="row">
            <span class="k">发起</span>
            <span class="muted">{formatTs(it.created_at_ms)} · {shortId(it.id)}</span>
          </div>
          <div class="actions">
            <button class="allow" disabled={busy === it.id} onclick={() => decide(it.id, "allow", false)}>
              批准
            </button>
            <button class="allow" disabled={busy === it.id} onclick={() => decide(it.id, "allow", true)}>
              批准并记忆
            </button>
            <button class="deny" disabled={busy === it.id} onclick={() => decide(it.id, "deny", false)}>
              拒绝
            </button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  <div class="card">
    <h3>自动审批 <small class="muted">勾选后命中的待批请求自动放行</small></h3>
    <p class="card-desc">
      人工盯着会话时（如接管中），预先声明可放行的敏感操作族；命中的
      <code>cap.request</code> 立即自动批准（单次，不产生永久授权），并照常进
      journal 审计。勾选存在本浏览器，默认全不勾。
    </p>
    <div class="rule-list">
      {#each AUTO_APPROVE_RULES as r (r.id)}
        <label class="rule">
          <input
            type="checkbox"
            checked={enabled.has(r.id)}
            onchange={(e) => onToggle(r.id, (e.currentTarget as HTMLInputElement).checked)}
            data-testid="auto-rule-{r.id}"
          />
          <span class="rule-text">
            <span class="mono">{r.label}</span>
            <small class="muted">{r.hint}</small>
          </span>
        </label>
      {/each}
    </div>
  </div>
</div>

<style>
  .rule-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .rule {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 7px 8px;
    border-radius: 8px;
    cursor: pointer;
  }

  .rule:hover {
    background: var(--surface-2);
  }

  .rule input {
    margin-top: 3px;
  }

  .rule-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    font-size: 12.5px;
  }
</style>
