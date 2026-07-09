// Console 登录客户端（gateway /auth/*）。
// fetch 经工厂注入，便于 vitest 全覆盖，无需真实网络。

export interface AuthProviders {
  password: boolean;
  microsoft: boolean;
}

export type FetchLike = (url: string, init?: RequestInit) => Promise<Response>;

const defaultFetch: FetchLike = (url, init) => fetch(url, init);

/** 探测已启用的登录方式；网关较旧（无 /auth/*）时视为全部关闭。 */
export async function fetchProviders(f: FetchLike = defaultFetch): Promise<AuthProviders> {
  try {
    const res = await f("/auth/providers");
    if (!res.ok) return { password: false, microsoft: false };
    const body = (await res.json()) as Partial<AuthProviders>;
    return { password: body.password === true, microsoft: body.microsoft === true };
  } catch {
    return { password: false, microsoft: false };
  }
}

/** 当前会话主体；无会话返回 null。 */
export async function fetchMe(f: FetchLike = defaultFetch): Promise<string | null> {
  try {
    const res = await f("/auth/me");
    if (!res.ok) return null;
    const body = (await res.json()) as { subject?: unknown };
    return typeof body.subject === "string" ? body.subject : null;
  } catch {
    return null;
  }
}

/** 用户名密码登录；成功返回 subject，失败抛 Error（含可展示消息）。 */
export async function loginPassword(
  username: string,
  password: string,
  f: FetchLike = defaultFetch,
): Promise<string> {
  const res = await f("/auth/login", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  if (res.status === 401) throw new Error("用户名或密码错误");
  if (res.status === 501) throw new Error("网关未启用密码登录");
  if (!res.ok) throw new Error(`登录失败（HTTP ${res.status}）`);
  const body = (await res.json()) as { subject?: unknown };
  return typeof body.subject === "string" ? body.subject : username;
}

export async function logout(f: FetchLike = defaultFetch): Promise<void> {
  try {
    await f("/auth/logout", { method: "POST" });
  } catch {
    // 网络失败时本地状态照常清理；会话最长 12h 自然过期
  }
}

/** `?login_error=…`（OAuth 回调失败）→ 可展示文案；无错误返回 null。 */
export function loginErrorMessage(search: string): string | null {
  const q = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const code = q.get("login_error");
  if (!code) return null;
  const known: Record<string, string> = {
    provider_error: "Microsoft 登录被取消或出错",
    state_mismatch: "登录状态校验失败，请重试",
    missing_code: "Microsoft 未返回授权码",
    exchange_failed: "授权码换取失败，请检查网关日志",
    not_allowed: "该账号不在允许名单内",
  };
  return known[code] ?? `登录失败：${code}`;
}
