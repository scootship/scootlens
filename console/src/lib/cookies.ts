// 把浏览器扩展（Cookie-Editor / EditThisCookie 等）导出的 cookie JSON +
// 可选的 localStorage，转成 ScootLens 的 StateBundle。
//
// 键名前缀与 driver 的 export_state 完全一致：
//   cookie:<name> → { value, domain, path, secure, httpOnly }
//   storage:<key> → <value>
//
// 纯函数、无副作用，便于单测；实际导入由 ConsoleApi.stateImport 走 state.import。
//
// 注意：浏览器安全模型禁止网页读取其它站点的 cookie（尤其 httpOnly），所以
// console 无法自动抓取——用户须从有 cookie 权限的扩展导出后粘贴。httpOnly 的
// 会话 cookie 正是靠扩展导出才拿得到（`document.cookie` 读不到）。

export interface StateBundle {
  entries: Record<string, unknown>;
}

export interface ParseResult {
  bundle: StateBundle;
  /** cookie 条数。 */
  cookies: number;
  /** 其中 httpOnly 的条数（会话 cookie 的关键指标）。 */
  httpOnly: number;
  /** localStorage 键数。 */
  storage: number;
}

interface RawCookie {
  name?: unknown;
  value?: unknown;
  domain?: unknown;
  path?: unknown;
  secure?: unknown;
  httpOnly?: unknown;
  // EditThisCookie 用驼峰 httpOnly；有的工具用 http_only。两者都容错。
  http_only?: unknown;
}

function asString(v: unknown, fallback = ""): string {
  return typeof v === "string" ? v : fallback;
}

function asBool(v: unknown): boolean {
  return v === true;
}

/** 从任意 JSON 文本里取出 cookie 数组（容错扩展导出的几种外形）。 */
function extractCookieArray(parsed: unknown): RawCookie[] {
  if (Array.isArray(parsed)) return parsed as RawCookie[];
  if (parsed && typeof parsed === "object") {
    const obj = parsed as Record<string, unknown>;
    // 有的导出包成 { cookies: [...] } 或 { entries: [...] }
    for (const key of ["cookies", "entries"]) {
      if (Array.isArray(obj[key])) return obj[key] as RawCookie[];
    }
  }
  throw new Error("cookie JSON 应是一个数组（Cookie-Editor「Export」的格式）");
}

/** localStorage：接受 {k:v} 对象、[[k,v]] 数组、或 Object.entries 的字符串形。 */
function extractStorage(parsed: unknown): [string, unknown][] {
  if (Array.isArray(parsed)) {
    return parsed
      .filter((p) => Array.isArray(p) && typeof p[0] === "string")
      .map((p) => [p[0] as string, p[1]]);
  }
  if (parsed && typeof parsed === "object") {
    return Object.entries(parsed as Record<string, unknown>);
  }
  throw new Error("localStorage 应是对象 {键:值} 或 [[键,值]] 数组");
}

/**
 * 组装 StateBundle。
 * @param cookiesText 扩展导出的 cookie JSON（必填）。
 * @param storageText 可选的 localStorage JSON。
 *
 * 同名不同域的 cookie 会按 `cookie:<name>` 折叠（后者覆盖），与 driver 的
 * export_state 键约定一致；单站点导入不受影响。
 */
export function buildStateBundle(cookiesText: string, storageText = ""): ParseResult {
  let parsedCookies: unknown;
  try {
    parsedCookies = JSON.parse(cookiesText);
  } catch (e) {
    throw new Error(`cookie JSON 解析失败：${e instanceof Error ? e.message : String(e)}`);
  }

  const raw = extractCookieArray(parsedCookies);
  const entries: Record<string, unknown> = {};
  let cookies = 0;
  let httpOnly = 0;

  for (const c of raw) {
    const name = asString(c.name);
    if (!name) continue;
    const isHttpOnly = asBool(c.httpOnly) || asBool(c.http_only);
    entries[`cookie:${name}`] = {
      value: asString(c.value),
      domain: asString(c.domain),
      path: asString(c.path, "/"),
      secure: asBool(c.secure),
      httpOnly: isHttpOnly,
    };
    cookies += 1;
    if (isHttpOnly) httpOnly += 1;
  }

  if (cookies === 0) {
    throw new Error("没有解析到任何 cookie（检查是否粘贴了正确的导出内容）");
  }

  let storage = 0;
  const trimmed = storageText.trim();
  if (trimmed) {
    let parsedStorage: unknown;
    try {
      parsedStorage = JSON.parse(trimmed);
    } catch (e) {
      throw new Error(
        `localStorage JSON 解析失败：${e instanceof Error ? e.message : String(e)}`,
      );
    }
    for (const [k, v] of extractStorage(parsedStorage)) {
      entries[`storage:${k}`] = typeof v === "string" ? v : JSON.stringify(v);
      storage += 1;
    }
  }

  return { bundle: { entries }, cookies, httpOnly, storage };
}
