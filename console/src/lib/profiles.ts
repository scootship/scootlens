// 已导入/用过的 profile 名字，记在浏览器本地，让「新开会话」从下拉里选而不是手输。
//
// 内核侧的权威列表走 `state.list`（namespace=profiles，ADR-0011）；这里的
// localStorage 记忆是补充：连不上内核时下拉仍可用，且能记住尚未导入过状态、
// 只是 spawn 用过的名字。Settings 页加载时会把内核列表并入本地记忆。

const KEY = "scootlens.profiles";

/** 最小的 storage 抽象，便于在 node 测试环境注入假实现。 */
export interface ProfileStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function resolve(store?: ProfileStore): ProfileStore | null {
  if (store) return store;
  try {
    if (typeof localStorage !== "undefined") return localStorage;
  } catch {
    // 某些环境（隐私模式/跨域）访问 localStorage 会抛异常。
  }
  return null;
}

function read(store: ProfileStore | null): string[] {
  if (!store) return [];
  try {
    const raw = store.getItem(KEY);
    if (!raw) return [];
    const arr: unknown = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    return arr.filter((x): x is string => typeof x === "string" && x.trim().length > 0);
  } catch {
    return [];
  }
}

function write(store: ProfileStore | null, names: string[]): string[] {
  const sorted = [...names].sort((a, b) => a.localeCompare(b));
  if (store) {
    try {
      store.setItem(KEY, JSON.stringify(sorted));
    } catch {
      // 写失败（配额/隐私模式）时静默降级，返回值仍可用于本次渲染。
    }
  }
  return sorted;
}

/** 列出记住的 profile 名（已去重、排序）。 */
export function listProfiles(store?: ProfileStore): string[] {
  return read(resolve(store));
}

/** 记住一个 profile 名（去重）。返回更新后的完整列表。 */
export function rememberProfile(name: string, store?: ProfileStore): string[] {
  const n = name.trim();
  const s = resolve(store);
  const cur = read(s);
  if (!n || cur.includes(n)) return cur.length ? write(s, cur) : cur;
  return write(s, [...cur, n]);
}

/** 忘记一个 profile 名。返回更新后的完整列表。 */
export function forgetProfile(name: string, store?: ProfileStore): string[] {
  const s = resolve(store);
  const next = read(s).filter((p) => p !== name.trim());
  return write(s, next);
}
