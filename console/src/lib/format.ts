// 纯展示辅助：时间戳、作用域、journal 种类、错误码。无副作用，便于单测全覆盖。

/** unix 毫秒 → `YYYY-MM-DD HH:MM:SS`（UTC）。非法输入回退 `—`。 */
export function formatTs(ms: unknown): string {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) return "—";
  const iso = new Date(ms).toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 19)}`;
}

/** 作用域可能是字符串或 `{domain,action,origin}` 结构，统一成可读串。 */
export function scopeLabel(scope: unknown): string {
  if (typeof scope === "string") return scope;
  if (scope && typeof scope === "object") {
    const o = scope as Record<string, unknown>;
    const segs = Array.isArray(o.segments)
      ? (o.segments as unknown[]).map(String).join(":")
      : [o.domain, o.action].filter(Boolean).map(String).join(":");
    const origin = typeof o.origin === "string" ? `@${o.origin}` : "";
    return segs ? `${segs}${origin}` : JSON.stringify(scope);
  }
  return String(scope ?? "");
}

const KIND_LABELS: Record<string, string> = {
  call: "→ 调用",
  result: "✓ 成功",
  deny: "✗ 拒绝",
  approval: "⚖ 审批",
};

export function kindLabel(kind: unknown): string {
  const k = String(kind ?? "").toLowerCase();
  return KIND_LABELS[k] ?? k ?? "?";
}

/** journal 种类 → 语义色（CSS class 后缀）。 */
export function kindTone(kind: unknown): "info" | "ok" | "danger" | "warn" | "muted" {
  switch (String(kind ?? "").toLowerCase()) {
    case "call":
      return "info";
    case "result":
      return "ok";
    case "deny":
      return "danger";
    case "approval":
      return "warn";
    default:
      return "muted";
  }
}

/** 截断长标识符：`apr-1234abcd…`。 */
export function shortId(id: unknown, keep = 10): string {
  const s = String(id ?? "");
  return s.length > keep ? `${s.slice(0, keep)}…` : s;
}

const ABI_CODE_LABELS: Record<string, string> = {
  E_CAP_DENIED: "能力不足 / 被策略拒绝",
  E_APPROVAL_PENDING: "等待人工审批",
  E_QUOTA: "超出限速配额",
  E_UNSUPPORTED: "当前引擎不支持",
  E_INVALID_ARG: "参数错误",
};

export function abiCodeLabel(code: unknown): string {
  const c = String(code ?? "");
  return ABI_CODE_LABELS[c] ?? c;
}
