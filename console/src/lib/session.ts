// Session 页逻辑：语义快照文本解析（输入注入的元素清单）与 screencast 节拍。
// 纯函数，便于 vitest 全覆盖；Svelte 组件仅做展示。

/** 快照文本中的一个可交互元素。 */
export interface SnapshotElement {
  /** 缩进层级（两空格 = 1 级）。 */
  depth: number;
  role: string;
  name: string;
  value?: string;
  /** 元素引用（如 `s3e17`）；非交互节点为 undefined。 */
  ref?: string;
}

/**
 * 解析 `view.snapshot` 的紧凑文本（`- role "name" = "value" [ref]`）。
 * 容错跳过畸形行；`truncated` 尾标记行忽略。
 */
export function parseSnapshotText(text: string): SnapshotElement[] {
  const out: SnapshotElement[] = [];
  for (const line of text.split("\n")) {
    const m = /^(\s*)- (\S+) "((?:[^"\\]|\\.)*)"(?: = "((?:[^"\\]|\\.)*)")?(?: \[([a-z0-9]+)\])?\s*$/.exec(
      line,
    );
    if (!m) continue;
    const el: SnapshotElement = {
      depth: Math.floor(m[1].length / 2),
      role: m[2],
      name: m[3],
    };
    if (m[4] !== undefined) el.value = m[4];
    if (m[5] !== undefined) el.ref = m[5];
    out.push(el);
  }
  return out;
}

/** 仅可交互元素（带 ref）。 */
export function interactive(elements: SnapshotElement[]): SnapshotElement[] {
  return elements.filter((e) => e.ref !== undefined);
}

/** 角色是否接受文本输入（Type 按钮可用性）。 */
export function acceptsText(role: string): boolean {
  return ["textbox", "searchbox", "combobox", "textarea", "input"].includes(role.toLowerCase());
}

/** screencast 轮询间隔（毫秒）。挂起/终止进程返回 0 = 停止轮询。 */
export function screencastInterval(procState: string | null | undefined): number {
  switch ((procState ?? "").toLowerCase()) {
    case "running":
      return 500;
    default:
      return 0;
  }
}

/** takeover 按钮状态机：无接管 → 可接管；自己持有 → 可归还；他人持有 → 只读提示。 */
export type TakeoverView =
  | { kind: "idle" }
  | { kind: "held-by-me" }
  | { kind: "held-by-other"; holder: string };

export function takeoverView(holder: string | null, self: string): TakeoverView {
  if (!holder) return { kind: "idle" };
  if (holder === self) return { kind: "held-by-me" };
  return { kind: "held-by-other", holder };
}
