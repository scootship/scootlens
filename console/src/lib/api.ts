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
}
