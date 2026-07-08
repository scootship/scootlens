// Replay 播放器逻辑：回放包解析、哈希链离线验证（WebCrypto）、时间线/帧对齐。
// 与内核 journal 同构：hash = sha256(prev + raw)，prev 链接前行 hash。

import type { JournalEntry } from "./journal";

export interface ReplayLine {
  seq: number;
  prev: string;
  hash: string;
  raw: string;
}

export interface ReplayFrame {
  ts_ms: number;
  format: string;
  data_base64: string;
}

export interface ReplayBundle {
  format_version: number;
  pid: string;
  engine: string;
  exported_at_ms: number;
  journal: ReplayLine[];
  frames: ReplayFrame[];
}

/** 链验证报告。 */
export interface ChainReport {
  ok: boolean;
  checked: number;
  /** 首个断链/篡改行的 seq（ok 时为 null）。 */
  brokenAt: number | null;
  reason: string | null;
}

/** 解析回放包 JSON（文件或 obs.replay.export 返回）。畸形输入抛 Error。 */
export function parseBundle(raw: unknown): ReplayBundle {
  const o = (typeof raw === "string" ? JSON.parse(raw) : raw) as Record<string, unknown>;
  if (!o || typeof o !== "object") throw new Error("replay bundle must be an object");
  if (o.format_version !== 1) {
    throw new Error(`unsupported replay format_version: ${String(o.format_version)}`);
  }
  if (typeof o.pid !== "string" || !Array.isArray(o.journal)) {
    throw new Error("replay bundle missing pid/journal");
  }
  const journal: ReplayLine[] = [];
  for (const l of o.journal) {
    const r = l as Record<string, unknown>;
    if (
      typeof r.seq === "number" &&
      typeof r.prev === "string" &&
      typeof r.hash === "string" &&
      typeof r.raw === "string"
    ) {
      journal.push({ seq: r.seq, prev: r.prev, hash: r.hash, raw: r.raw });
    }
  }
  const frames: ReplayFrame[] = [];
  if (Array.isArray(o.frames)) {
    for (const f of o.frames) {
      const r = f as Record<string, unknown>;
      if (typeof r.ts_ms === "number" && typeof r.data_base64 === "string") {
        frames.push({
          ts_ms: r.ts_ms,
          format: typeof r.format === "string" ? r.format : "png",
          data_base64: r.data_base64,
        });
      }
    }
  }
  return {
    format_version: 1,
    pid: o.pid,
    engine: typeof o.engine === "string" ? o.engine : "?",
    exported_at_ms: typeof o.exported_at_ms === "number" ? o.exported_at_ms : 0,
    journal,
    frames,
  };
}

async function sha256hex(text: string): Promise<string> {
  const data = new TextEncoder().encode(text);
  const digest = await crypto.subtle.digest("SHA-256", data);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

/**
 * 离线重放验证链段：
 * - 首行之后每行 `prev` 必须等于前行 `hash`（链接性）
 * - 每行 `hash` 必须等于 sha256(prev + raw)（完整性）
 */
export async function verifyChain(journal: ReplayLine[]): Promise<ChainReport> {
  let prevHash: string | null = null;
  for (const line of journal) {
    if (prevHash !== null && line.prev !== prevHash) {
      return {
        ok: false,
        checked: journal.length,
        brokenAt: line.seq,
        reason: "链断裂：prev 与前行 hash 不一致",
      };
    }
    const expect = await sha256hex(line.prev + line.raw);
    if (expect !== line.hash) {
      return {
        ok: false,
        checked: journal.length,
        brokenAt: line.seq,
        reason: "hash 不匹配：行内容被修改",
      };
    }
    prevHash = line.hash;
  }
  return { ok: true, checked: journal.length, brokenAt: null, reason: null };
}

/** 时间线条目：解析 raw 为 journal entry，标记是否属于目标 pid。 */
export interface TimelineItem {
  entry: JournalEntry;
  ofPid: boolean;
}

/** 链段 → 时间线（旧→新）；raw 不可解析的行跳过。 */
export function timeline(bundle: ReplayBundle): TimelineItem[] {
  const out: TimelineItem[] = [];
  for (const line of bundle.journal) {
    try {
      const e = JSON.parse(line.raw) as JournalEntry;
      if (typeof e.seq !== "number") continue;
      out.push({ entry: e, ofPid: e.pid === bundle.pid });
    } catch {
      // 跳过畸形行（验证器会单独报告链完整性）
    }
  }
  return out;
}

/** 给定时间点，找到不晚于该时刻的最近一帧（无帧返回 null）。 */
export function frameAt(frames: ReplayFrame[], tsMs: number): ReplayFrame | null {
  let best: ReplayFrame | null = null;
  for (const f of frames) {
    if (f.ts_ms <= tsMs && (best === null || f.ts_ms > best.ts_ms)) best = f;
  }
  return best;
}

/** 帧 → data URL。 */
export function frameUrl(frame: ReplayFrame): string {
  return `data:image/${frame.format};base64,${frame.data_base64}`;
}
