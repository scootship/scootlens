// RPC 错误 → 面向用户的可读文案。
// 典型场景：旧版 scootlensd 不认识新方法（如 act.point.click）返回
// "method not found"，直接透出会让用户以为是点击逻辑坏了。

import { RpcError } from "./rpc";

/** JSON-RPC 标准 method-not-found 错误码。 */
const METHOD_NOT_FOUND = -32601;

function isMethodMissing(e: unknown): boolean {
  if (!(e instanceof RpcError)) return false;
  if (e.code === METHOD_NOT_FOUND) return true;
  return /method not found/i.test(e.message) || e.abiCode === "E_UNSUPPORTED";
}

/** 错误 → 展示文案；method 用于补充上下文（如升级提示）。 */
export function friendlyError(e: unknown, method?: string): string {
  if (isMethodMissing(e) && method) {
    return `内核不支持 ${method}（守护进程版本过旧，请升级 scootlensd 后重试）`;
  }
  if (e instanceof RpcError) {
    const code = e.abiCode ? `${e.abiCode}: ` : "";
    return `${code}${e.message}`;
  }
  return e instanceof Error ? e.message : String(e);
}
