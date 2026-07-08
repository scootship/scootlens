// 连接参数：支持 `?token=…&connect=1` 快速接入（本地/e2e 便利；
// 令牌本就经 URL query 完成 WS 握手，此处不引入新的暴露面）。

export interface ConnectParams {
  token: string | null;
  /** `connect=1` 时自动发起连接。 */
  auto: boolean;
  /** 覆盖 gateway 基址（默认取页面 host）。 */
  base: string | null;
}

export function parseConnectParams(search: string): ConnectParams {
  const q = new URLSearchParams(search.startsWith("?") ? search.slice(1) : search);
  const token = q.get("token");
  return {
    token: token && token.trim() ? token.trim() : null,
    auto: q.get("connect") === "1" && !!token,
    base: q.get("base"),
  };
}

/** 页面 location → 默认 WS 基址。 */
export function defaultBase(protocol: string, host: string): string {
  if (!host) return "ws://127.0.0.1:9910";
  return `${protocol === "https:" ? "wss:" : "ws:"}//${host}`;
}
