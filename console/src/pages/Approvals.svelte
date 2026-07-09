<script lang="ts">
  import type { ConsoleApi, PendingApproval, ApprovalDecision } from "../lib/api";
  import { scopeLabel, formatTs, shortId } from "../lib/format";
  import { friendlyError } from "../lib/errors";

  let { api, pulse }: { api: ConsoleApi; pulse: number } = $props();

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
