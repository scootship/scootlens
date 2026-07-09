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

export interface LoginFields {
  username?: SnapshotElement;
  password?: SnapshotElement;
}

function fieldText(el: SnapshotElement): string {
  return `${el.role} ${el.name} ${el.value ?? ""}`.toLowerCase();
}

function passwordScore(el: SnapshotElement): number {
  const text = fieldText(el);
  if (/(password|passcode|passwd|密码|口令)/i.test(text)) return 3;
  return 0;
}

function usernameScore(el: SnapshotElement): number {
  const text = fieldText(el);
  if (/(username|user name|email|login|account|用户名|账号|邮箱|邮件)/i.test(text)) return 3;
  if (/(user|mail)/i.test(text)) return 1;
  return 0;
}

/** 从语义元素中选出最可能的用户名/密码输入框；返回 ref-bearing 文本字段。 */
export function pickLoginFields(elements: SnapshotElement[]): LoginFields {
  const textInputs = elements.filter((e) => e.ref && acceptsText(e.role));
  const password = textInputs
    .map((el, index) => ({ el, index, score: passwordScore(el) }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.index - b.index)[0];

  const passwordIndex = password ? textInputs.indexOf(password.el) : -1;
  const username = textInputs
    .map((el, index) => ({
      el,
      index,
      score:
        el === password?.el
          ? -1
          : usernameScore(el) * 10 +
            (passwordIndex > index ? Math.max(0, 3 - (passwordIndex - index)) : 0),
    }))
    .filter((x) => x.score > 0)
    .sort((a, b) => b.score - a.score || a.index - b.index)[0];

  return { username: username?.el, password: password?.el };
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

/** 画面上一次点击换算出的归一化视口坐标（`act.point.click` 的 x_ratio/y_ratio）。 */
export interface ClickRatio {
  xRatio: number;
  yRatio: number;
}

/** 矩形（CSS 像素），左上角 (x,y) + 宽高。 */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * `object-fit: contain` 布局下，图片实际渲染内容在元素盒子内的矩形
 * （居中留白 letterbox 已扣除）。CSS 用的是 `contain`（`app.css` `.viewport img`），
 * 元素盒子尺寸与图片原始宽高比不一致时，四周会留白——点击换算必须先扣掉这部分，
 * 否则贴边点击会算出错误比例。尺寸非法时返回 null。
 */
export function containRect(
  naturalWidth: number,
  naturalHeight: number,
  boxWidth: number,
  boxHeight: number,
): Rect | null {
  if (!(naturalWidth > 0) || !(naturalHeight > 0) || !(boxWidth > 0) || !(boxHeight > 0)) {
    return null;
  }
  const scale = Math.min(boxWidth / naturalWidth, boxHeight / naturalHeight);
  const width = naturalWidth * scale;
  const height = naturalHeight * scale;
  return { x: (boxWidth - width) / 2, y: (boxHeight - height) / 2, width, height };
}

/**
 * 把点击事件在元素盒子内的偏移（`offsetX/offsetY`）换算成画面内容矩形
 * （见 [containRect]）内的 [0,1] 归一化视口比例；矩形之外（letterbox 留白区）
 * 裁剪到最近边缘，而不是丢弃——避免贴边点击因四舍五入落空。
 */
export function clickRatio(offsetX: number, offsetY: number, rect: Rect): ClickRatio | null {
  if (!(rect.width > 0) || !(rect.height > 0)) return null;
  const clamp01 = (v: number) => Math.min(1, Math.max(0, v));
  return {
    xRatio: clamp01((offsetX - rect.x) / rect.width),
    yRatio: clamp01((offsetY - rect.y) / rect.height),
  };
}
