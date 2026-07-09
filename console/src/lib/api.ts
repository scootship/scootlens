// 领域方法封装（docs/03-abi-spec.md 的子集）。保持极薄，纯委托 RpcClient.call，
// 便于用 stub client 单测。

import type { RpcClient } from "./rpc";
import { parseEntries, type JournalEntry } from "./journal";

export interface SysInfo {
  abi_version: string;
  kernel_version: string;
  engine: string;
  max_procs: number;
  running_procs: number;
  caps?: Record<string, unknown>;
}

export interface ProcInfo {
  pid: string;
  state: string;
  engine?: string;
  profile?: string;
  url?: string | null;
  title?: string | null;
}

export interface PendingApproval {
  id: string;
  subject: string;
  scope: unknown;
  method: string;
  params_summary?: unknown;
  reason?: string | null;
  created_at_ms: number;
}

export type ApprovalDecision = "allow" | "deny";

/** 回放包（obs.replay.export → bundle 字段；docs/03-abi-spec.md）。 */
export interface ReplayBundleWire {
  format_version: number;
  pid: string;
  engine: string;
  exported_at_ms: number;
  journal: unknown[];
  frames?: unknown[];
}

/** 网络请求日志条目（net.log）。 */
export interface NetLogEntry {
  url?: string;
  method?: string;
  allowed?: boolean;
  ts_ms?: number;
  [k: string]: unknown;
}

/** profile entry 的隐私摘要（state.read namespace=profiles；值绝不回流，ADR-0011）。 */
export interface ProfileEntryDigest {
  key: string;
  kind: "cookie" | "storage" | "other";
  value_bytes: number;
  domain?: string;
  path?: string;
  secure?: boolean;
  httpOnly?: boolean;
}

/** 只读取数组字段，容错非数组。 */
function asArray(v: unknown, key: string): unknown[] {
  if (v && typeof v === "object" && Array.isArray((v as Record<string, unknown>)[key])) {
    return (v as Record<string, unknown[]>)[key];
  }
  return [];
}

export class ConsoleApi {
  constructor(private readonly client: RpcClient) {}

  sysInfo(): Promise<SysInfo> {
    return this.client.call<SysInfo>("sys.info");
  }

  async procList(): Promise<ProcInfo[]> {
    const r = await this.client.call("proc.list");
    return asArray(r, "procs") as ProcInfo[];
  }

  async pending(): Promise<PendingApproval[]> {
    const r = await this.client.call("cap.pending");
    return asArray(r, "pending") as PendingApproval[];
  }

  approve(approvalId: string, decision: ApprovalDecision, remember = false): Promise<unknown> {
    return this.client.call("cap.approve", {
      approval_id: approvalId,
      decision,
      remember,
    });
  }

  async journal(limit = 100, pid?: string): Promise<JournalEntry[]> {
    const params: Record<string, unknown> = { limit };
    if (pid) params.pid = pid;
    const r = await this.client.call("obs.journal", params);
    return parseEntries((r as { entries?: unknown } | null)?.entries);
  }

  // ---------- P4：Session / Inspector / Replay / Settings ----------

  /** 连接级事件订阅（gateway 会话语义）；返回 sub_id。 */
  async subscribe(pid?: string, topics: string[] = []): Promise<string> {
    const params: Record<string, unknown> = { topics };
    if (pid) params.pid = pid;
    const r = await this.client.call<{ sub_id: string }>("evt.subscribe", params);
    return r.sub_id;
  }

  async procSpawn(profile?: string): Promise<string> {
    const params: Record<string, unknown> = {};
    if (profile) params.profile = profile;
    const r = await this.client.call<{ pid: string }>("proc.spawn", params);
    return r.pid;
  }

  procKill(pid: string): Promise<unknown> {
    return this.client.call("proc.kill", { pid });
  }

  navGoto(pid: string, url: string): Promise<unknown> {
    return this.client.call("nav.goto", { pid, url });
  }

  /** 截图 → data URL（screencast 帧）。 */
  async screenshot(pid: string): Promise<string> {
    const r = await this.client.call<{ format: string; data_base64: string }>(
      "view.screenshot",
      { pid },
    );
    return `data:image/${r.format};base64,${r.data_base64}`;
  }

  /** 语义快照紧凑文本。 */
  async snapshotText(pid: string): Promise<string> {
    const r = await this.client.call<{ text: string }>("view.snapshot", { pid });
    return r.text ?? "";
  }

  actClick(pid: string, ref: string): Promise<unknown> {
    return this.client.call("act.click", { pid, ref });
  }

  /** 接管期间坐标点击（归一化视口比例 [0,1]）；仅当调用者持有接管时内核放行。 */
  actClickAt(pid: string, xRatio: number, yRatio: number): Promise<unknown> {
    return this.client.call("act.point.click", { pid, x_ratio: xRatio, y_ratio: yRatio });
  }

  actType(pid: string, ref: string, text: string): Promise<unknown> {
    return this.client.call("act.type", { pid, ref, text });
  }

  actTypeVault(pid: string, ref: string, vaultRef: string): Promise<unknown> {
    return this.client.call("act.type", { pid, ref, vault_ref: vaultRef });
  }

  actPress(pid: string, keys: string): Promise<unknown> {
    return this.client.call("act.press", { pid, keys });
  }

  takeoverStart(pid: string): Promise<unknown> {
    return this.client.call("act.takeover.start", { pid });
  }

  takeoverEnd(pid: string): Promise<unknown> {
    return this.client.call("act.takeover.end", { pid });
  }

  async netLog(pid: string, limit = 50): Promise<NetLogEntry[]> {
    const r = await this.client.call("net.log", { pid, limit });
    return asArray(r, "entries") as NetLogEntry[];
  }

  async replayExport(pid: string, journalLimit = 1000): Promise<ReplayBundleWire> {
    const r = await this.client.call<{ bundle: ReplayBundleWire }>("obs.replay.export", {
      pid,
      journal_limit: journalLimit,
    });
    return r.bundle;
  }

  capList(): Promise<{ subject: string; scopes: string[] }> {
    return this.client.call("cap.list");
  }

  capGrant(subject: string, scope: string): Promise<unknown> {
    return this.client.call("cap.grant", { subject, scope });
  }

  capRevoke(subject: string, scope: string): Promise<unknown> {
    return this.client.call("cap.revoke", { subject, scope });
  }

  /** vault 单向写入（只写不读）；返回后仅显示 vault_ref 句柄。 */
  vaultWrite(name: string, secret: string): Promise<unknown> {
    return this.client.call("state.write", { namespace: "vault", key: name, value: secret });
  }

  /** 只列出 vault_ref 名称，不返回 secret。 */
  async vaultList(): Promise<string[]> {
    const r = await this.client.call("state.list", { namespace: "vault" });
    return asArray(r, "names") as string[];
  }

  /** 删除一条 vault 凭据（state.delete，🔒）。历史 journal 的脱敏不回收。 */
  vaultDelete(name: string): Promise<unknown> {
    return this.client.call("state.delete", { namespace: "vault", key: name });
  }

  /** 把登录会话（cookie + localStorage）导入 profile，后续 spawn 该 profile 即带登录态。
   *  state.import 是敏感操作（🔒）：admin 令牌自动放行，普通令牌会进 Approvals 待批。 */
  stateImport(profile: string, bundle: unknown): Promise<unknown> {
    return this.client.call("state.import", { profile, state: bundle });
  }

  /** 内核里已导入的 profile 名（state.list namespace=profiles，只有名字）。 */
  async profileList(): Promise<string[]> {
    const r = await this.client.call("state.list", { namespace: "profiles" });
    return asArray(r, "names") as string[];
  }

  /** profile 内容的隐私摘要：entry 键名、cookie 的域/标志、值字节数——
   *  值绝不回流（ADR-0011）。敏感操作（🔒）。 */
  async profileDigest(name: string): Promise<ProfileEntryDigest[]> {
    const r = await this.client.call("state.read", { namespace: "profiles", key: name });
    return asArray(r, "entries") as ProfileEntryDigest[];
  }

  /** 删除整个 profile（缺省）或其中单条 entry（state.delete，🔒）。 */
  profileDelete(name: string, entry?: string): Promise<unknown> {
    const params: Record<string, unknown> = { namespace: "profiles", key: name };
    if (entry) params.entry = entry;
    return this.client.call("state.delete", params);
  }

  /** 导出运行中会话的完整状态束（cookies + storage）；配合 stateImport
   *  实现「接管登录 → 存为 profile → 新会话复用」。敏感操作（🔒）。 */
  async stateExport(pid: string): Promise<unknown> {
    const r = await this.client.call<{ state: unknown }>("state.export", { pid });
    return r.state;
  }

  netRulesGet(pid?: string): Promise<unknown> {
    const params: Record<string, unknown> = {};
    if (pid) params.pid = pid;
    return this.client.call("net.rules.get", params);
  }

  netRulesSet(rules: unknown, pid?: string): Promise<unknown> {
    const params: Record<string, unknown> =
      rules && typeof rules === "object" ? { ...(rules as Record<string, unknown>) } : {};
    if (pid) params.pid = pid;
    return this.client.call("net.rules.set", params);
  }
}
