// 关停 globalSetup 启动的 scootlensd。

import { readFileSync, rmSync } from "node:fs";
import { STATE_FILE } from "./global-setup";

export default async function globalTeardown(): Promise<void> {
  const child = globalThis.__scootlensd;
  if (child && !child.killed) {
    child.kill("SIGTERM");
  } else {
    // 兜底：跨进程场景按状态文件回收
    try {
      const { pid } = JSON.parse(readFileSync(STATE_FILE, "utf8")) as { pid?: number };
      if (pid) process.kill(pid, "SIGTERM");
    } catch {
      // 已退出
    }
  }
  try {
    rmSync(STATE_FILE);
  } catch {
    // 忽略
  }
}
