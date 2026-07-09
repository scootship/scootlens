// 事件流展示逻辑：topic 分族着色 + 载荷压缩成可读键值摘要。
// 原始 JSON 保留在明细里，列表行只呈现关键字段，避免长串 JSON 折行成噪声。

export interface ConsoleEvent {
  seq: number;
  topic: string;
  text: string;
}

/** topic 前缀 → 视觉基调。 */
export function topicTone(topic: string): "info" | "ok" | "warn" | "danger" | "muted" {
  if (topic.startsWith("proc.")) return "ok";
  if (topic.startsWith("act.")) return "warn";
  if (topic.startsWith("net.")) return "danger";
  if (topic.startsWith("cap.") || topic.startsWith("approval")) return "danger";
  if (topic.startsWith("nav.") || topic.startsWith("view.")) return "info";
  return "muted";
}

/** 摘要里无信息量的载荷字段（已单列展示）。 */
const OMIT_KEYS = new Set(["seq", "topic"]);
/** 摘要最多展示的字段数。 */
const MAX_FIELDS = 4;
/** 单字段值最长展示长度。 */
const MAX_VALUE = 48;

function compact(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string") return v.length > MAX_VALUE ? `${v.slice(0, MAX_VALUE)}…` : v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  const s = JSON.stringify(v) ?? "?";
  return s.length > MAX_VALUE ? `${s.slice(0, MAX_VALUE)}…` : s;
}

/** 事件载荷 → `k=v` 摘要片段（最多 MAX_FIELDS 个字段）。 */
export function summarizeEvent(text: string): { pid: string | null; fields: string[] } {
  let payload: unknown;
  try {
    payload = JSON.parse(text);
  } catch {
    return { pid: null, fields: [compact(text)] };
  }
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    return { pid: null, fields: [compact(payload)] };
  }
  const obj = payload as Record<string, unknown>;
  const pid = typeof obj.pid === "string" ? obj.pid : null;
  const fields = Object.entries(obj)
    .filter(([k]) => !OMIT_KEYS.has(k) && k !== "pid")
    .slice(0, MAX_FIELDS)
    .map(([k, v]) => `${k}=${compact(v)}`);
  return { pid, fields };
}
