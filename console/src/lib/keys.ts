// 键盘透传：浏览器 KeyboardEvent → `act.press` 键名（接管模式画面输入）。
// 纯函数，便于 vitest 全覆盖；组件里只做监听与调用。

/** driver 支持的命名控制键（scootlens-driver-chromium `key_info`）。 */
const NAMED_KEYS = new Set([
  "Enter",
  "Tab",
  "Escape",
  "Backspace",
  "Delete",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "ArrowDown",
  "ArrowUp",
  "ArrowLeft",
  "ArrowRight",
]);

/** KeyboardEvent 的最小结构契约（便于测试构造）。 */
export interface KeyLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

/**
 * 事件 → `act.press` 键名；不该透传的按键返回 null：
 * - 带 Ctrl/Meta/Alt 的组合键（保留给用户浏览器自身，如 Cmd+R/Cmd+C）
 * - 纯修饰键（Shift/CapsLock 等，Shift 的效果已体现在 `key` 的字符上）
 * - driver 不支持的功能键（F1-12 等）
 */
export function pressKeyFor(ev: KeyLike): string | null {
  if (ev.ctrlKey || ev.metaKey || ev.altKey) return null;
  if (NAMED_KEYS.has(ev.key)) return ev.key;
  // 单个可打印字符（含空格 " "、中文等宽字符）
  if ([...ev.key].length === 1) return ev.key;
  return null;
}
