// journal 完整性检查（客户端侧）。obs.journal 以新→旧序返回窗口，每条带
// 单调 `seq` 与链上 `hash`。P2 客户端做轻量自证：seq 连续性 + hash 存在性。
// 完整哈希链重放（prev+raw）需 obs.replay.export（P4）。

export interface JournalEntry {
  seq: number;
  ts_ms: number;
  kind: string;
  subject: string;
  method: string;
  pid?: string | null;
  hash?: string;
  detail?: unknown;
}

export interface IntegrityReport {
  /** 窗口内无 seq 缺口/乱序，且每条都带 hash。 */
  ok: boolean;
  count: number;
  /** 缺失的 seq（相邻两条之间的空洞）。 */
  gaps: number[];
  /** 缺少 hash 的条目 seq。 */
  missingHash: number[];
}

/** 解析 obs.journal 的 `entries`，容错跳过畸形条目。 */
export function parseEntries(raw: unknown): JournalEntry[] {
  if (!Array.isArray(raw)) return [];
  const out: JournalEntry[] = [];
  for (const item of raw) {
    if (item && typeof item === "object" && "seq" in item) {
      const o = item as Record<string, unknown>;
      if (typeof o.seq === "number") {
        out.push({
          seq: o.seq,
          ts_ms: typeof o.ts_ms === "number" ? o.ts_ms : 0,
          kind: String(o.kind ?? ""),
          subject: String(o.subject ?? ""),
          method: String(o.method ?? ""),
          pid: typeof o.pid === "string" ? o.pid : null,
          hash: typeof o.hash === "string" ? o.hash : undefined,
          detail: o.detail,
        });
      }
    }
  }
  return out;
}

/**
 * 校验一个 journal 窗口（新→旧序）：
 * - seq 严格递减、步长为 1（无缺口、无重复、无乱序）
 * - 每条都带非空 hash
 * 注意：pid 过滤后的窗口会天然出现 seq 缺口，故仅对**未过滤**的完整窗口调用。
 */
export function checkIntegrity(entries: JournalEntry[]): IntegrityReport {
  const gaps: number[] = [];
  const missingHash: number[] = [];
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i];
    if (!e.hash) missingHash.push(e.seq);
    if (i > 0) {
      const prev = entries[i - 1].seq;
      // 新→旧：前一条 seq 应恰好比当前大 1。
      if (prev - e.seq !== 1) {
        const from = e.seq + 1;
        const to = prev - 1;
        for (let s = from; s <= to && gaps.length < 64; s++) gaps.push(s);
        if (prev <= e.seq) gaps.push(e.seq); // 乱序/重复
      }
    }
  }
  return {
    ok: gaps.length === 0 && missingHash.length === 0,
    count: entries.length,
    gaps,
    missingHash,
  };
}
