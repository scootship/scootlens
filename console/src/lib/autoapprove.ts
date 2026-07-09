// Console 侧自动审批：管理员预先勾选的作用域族，命中 `cap.request` 事件即
// 自动 `cap.approve(allow)`——把"人在场时逐条点批准"变成"人预先声明意图"。
//
// 安全边界：这是审批人（当前 console 连接）的用户空间自动化，不改内核语义——
// 审批仍由持有 cap:admin 的连接做出、进 journal 审计；勾选状态存浏览器本地，
// 不影响其它管理员。默认全不勾选。

import type { ProfileStore } from "./profiles";

/** 可自动批准的作用域族（与 SENSITIVE_SCOPES 对齐，按风险拆分）。 */
export interface AutoApproveRule {
  /** 作用域段前缀，如 `js:exec`。 */
  id: string;
  label: string;
  /** 风险提示（UI 副文案）。 */
  hint: string;
}

export const AUTO_APPROVE_RULES: AutoApproveRule[] = [
  { id: "js:exec", label: "js.exec 脚本执行", hint: "允许 Agent 在页面里跑任意 JS" },
  { id: "state:import", label: "state.import 导入登录态", hint: "写入 profile 供 spawn 复用" },
  { id: "state:export", label: "state.export 导出会话状态", hint: "读出 cookie 等登录态" },
  { id: "state:read", label: "state.read 读状态", hint: "读 cookies/storage 命名空间" },
  { id: "state:write", label: "state.write 写状态", hint: "写 cookies/storage（vault 除外）" },
  { id: "act:upload", label: "act.upload 文件上传", hint: "把沙箱文件递给页面" },
  { id: "act:takeover", label: "act.takeover 接管", hint: "其他主体申请接管会话" },
  { id: "net:rules", label: "net.rules 网络规则", hint: "调整出口放行/拦截" },
  { id: "vault:use", label: "vault:use 凭据注入", hint: "把 vault 凭据注入表单" },
  { id: "obs:replay", label: "obs.replay 回放导出", hint: "导出含画面帧的回放包" },
];

const KEY = "scootlens.autoapprove";

function resolve(store?: ProfileStore): ProfileStore | null {
  if (store) return store;
  try {
    if (typeof localStorage !== "undefined") return localStorage;
  } catch {
    // 隐私模式等场景访问 localStorage 会抛异常
  }
  return null;
}

/** 读取勾选的规则 id 集合。 */
export function listAutoApprove(store?: ProfileStore): Set<string> {
  const s = resolve(store);
  if (!s) return new Set();
  try {
    const raw = s.getItem(KEY);
    if (!raw) return new Set();
    const arr: unknown = JSON.parse(raw);
    if (!Array.isArray(arr)) return new Set();
    const known = new Set(AUTO_APPROVE_RULES.map((r) => r.id));
    return new Set(arr.filter((x): x is string => typeof x === "string" && known.has(x)));
  } catch {
    return new Set();
  }
}

/** 勾选/取消一条规则；返回更新后的集合。 */
export function toggleAutoApprove(id: string, on: boolean, store?: ProfileStore): Set<string> {
  const cur = listAutoApprove(store);
  if (on) cur.add(id);
  else cur.delete(id);
  const s = resolve(store);
  if (s) {
    try {
      s.setItem(KEY, JSON.stringify([...cur].sort((a, b) => a.localeCompare(b))));
    } catch {
      // 写失败静默降级：本次会话内仍生效
    }
  }
  return cur;
}

/** `cap.request` 事件的作用域是否命中勾选集合（按段前缀匹配，忽略 @origin）。 */
export function matchesAutoApprove(scope: string, enabled: ReadonlySet<string>): string | null {
  const body = scope.split("@")[0];
  const segs = body.split(":").filter(Boolean);
  for (const id of enabled) {
    const want = id.split(":");
    if (want.length <= segs.length && want.every((w, i) => w === segs[i])) return id;
  }
  return null;
}

/** `cap.request` 事件载荷（App 侧订阅回调解析用）。 */
export interface CapRequestEvent {
  approvalId: string;
  method: string;
  scope: string;
}

/** 从 evt.event 载荷中解析 cap.request；不是该主题返回 null。 */
export function parseCapRequest(text: string): CapRequestEvent | null {
  try {
    const obj = JSON.parse(text) as Record<string, unknown>;
    if (obj?.topic !== "cap.request") return null;
    const approvalId = obj.approval_id;
    if (typeof approvalId !== "string" || !approvalId) return null;
    return {
      approvalId,
      method: typeof obj.method === "string" ? obj.method : "?",
      scope: typeof obj.scope === "string" ? obj.scope : "",
    };
  } catch {
    return null;
  }
}
