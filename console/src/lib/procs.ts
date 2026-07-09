// 进程列表视图逻辑：排序 + terminated 过滤（Dashboard/选择器共用）。
// terminated 进程会在内核里保留（审计可回溯），UI 默认收起避免霸屏。

import type { ProcInfo } from "./api";

/** 状态排序权重：活跃在前。 */
const STATE_ORDER: Record<string, number> = {
  running: 0,
  suspended: 1,
  spawning: 2,
  terminated: 9,
};

function stateRank(state: string): number {
  return STATE_ORDER[state] ?? 5;
}

/** pid 自然序（p-2 < p-10）。 */
function pidCompare(a: string, b: string): number {
  const na = Number(a.replace(/\D+/g, ""));
  const nb = Number(b.replace(/\D+/g, ""));
  if (Number.isFinite(na) && Number.isFinite(nb) && na !== nb) return na - nb;
  return a.localeCompare(b);
}

/** 活跃优先 + pid 自然序。 */
export function sortProcs(procs: ProcInfo[]): ProcInfo[] {
  return [...procs].sort(
    (a, b) => stateRank(a.state) - stateRank(b.state) || pidCompare(a.pid, b.pid),
  );
}

export function isActive(p: ProcInfo): boolean {
  return p.state !== "terminated";
}

/** 视图切分：active（默认展示）与 terminated（计数 + 按需展开）。 */
export function splitProcs(procs: ProcInfo[]): {
  active: ProcInfo[];
  terminated: ProcInfo[];
} {
  const sorted = sortProcs(procs);
  return {
    active: sorted.filter(isActive),
    terminated: sorted.filter((p) => !isActive(p)),
  };
}

/** 选择器首选 pid：第一个活跃进程；全部终止则回退第一个。 */
export function preferredPid(procs: ProcInfo[]): string {
  const { active, terminated } = splitProcs(procs);
  return active[0]?.pid ?? terminated[0]?.pid ?? "";
}

/** 仅活跃进程的首选 pid（Session/Inspector：终止进程不自动选中）。 */
export function preferredActivePid(procs: ProcInfo[]): string {
  return splitProcs(procs).active[0]?.pid ?? "";
}

/** 选择器可选项：活跃进程 + （若当前选中的已终止）该项保留在末尾以免下拉跳变。 */
export function selectableProcs(procs: ProcInfo[], currentPid: string): ProcInfo[] {
  const { active, terminated } = splitProcs(procs);
  const current = terminated.find((p) => p.pid === currentPid);
  return current ? [...active, current] : active;
}

/** 状态 → 视觉基调（tag 样式）。 */
export function stateTone(state: string): "ok" | "warn" | "muted" | "info" {
  switch (state) {
    case "running":
      return "ok";
    case "suspended":
      return "warn";
    case "terminated":
      return "muted";
    default:
      return "info";
  }
}
