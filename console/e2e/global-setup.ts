// 启动 scootlensd（mock 引擎 + 托管 console/dist），解析令牌，写入状态文件。
// 二进制须先构建：cargo build -p scootlensd（CI 在 e2e job 内完成）。

import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PORT = 39231;
export const STATE_FILE = join(tmpdir(), "scootlens-console-e2e.json");

declare global {
  // eslint-disable-next-line no-var
  var __scootlensd: ChildProcess | undefined;
}

async function waitForTokens(child: ChildProcess): Promise<{ admin: string; agent: string }> {
  return new Promise((resolvePromise, reject) => {
    let buf = "";
    const timer = setTimeout(
      () => reject(new Error(`scootlensd did not print tokens in time; output so far:\n${buf}`)),
      30_000,
    );
    child.stdout?.on("data", (chunk: Buffer) => {
      buf += chunk.toString();
      const admin = /admin token: (slt1\.\S+)/.exec(buf);
      const agent = /token\[agent:e2e\]: (slt1\.\S+)/.exec(buf);
      const listening = buf.includes("listening on");
      if (admin && agent && listening) {
        clearTimeout(timer);
        resolvePromise({ admin: admin[1], agent: agent[1] });
      }
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`scootlensd exited early (code ${code}); output:\n${buf}`));
    });
  });
}

export default async function globalSetup(): Promise<void> {
  const repoRoot = resolve(HERE, "..", "..");
  const bin = process.env.SCOOTLENSD_BIN ?? join(repoRoot, "target", "debug", "scootlensd");
  const stateDir = mkdtempSync(join(tmpdir(), "scootlens-e2e-"));

  const child = spawn(
    bin,
    [
      "--engine",
      "mock",
      "--listen",
      `127.0.0.1:${PORT}`,
      "--console-dir",
      join(repoRoot, "console", "dist"),
      "--state-dir",
      stateDir,
      // 受限 agent 令牌：act@fixture.test 但无 act:takeover；js:exec 敏感 → 人工审批
      "--issue",
      "agent:e2e=proc:list,nav@fixture.test,view@fixture.test,act@fixture.test,js:exec@fixture.test",
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let stderr = "";
  child.stderr?.on("data", (c: Buffer) => (stderr += c.toString()));
  child.on("exit", (code) => {
    if (code !== null && code !== 0) console.error(`scootlensd stderr:\n${stderr}`);
  });

  const tokens = await waitForTokens(child);
  globalThis.__scootlensd = child;
  writeFileSync(STATE_FILE, JSON.stringify({ pid: child.pid, port: PORT, ...tokens }));
  process.env.SCOOTLENS_E2E_ADMIN = tokens.admin;
  process.env.SCOOTLENS_E2E_AGENT = tokens.agent;
  process.env.SCOOTLENS_E2E_PORT = String(PORT);
}
