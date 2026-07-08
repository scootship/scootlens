// Console UI e2e 关键路径（P4 门禁 #4）：
// 连接 → Dashboard spawn → Session screencast/接管（含 Agent 输入挂起→恢复）→
// Approvals 审批闭环 → Journal 完整性 → Replay 导出验链 → Settings 管理动作。
//
// Agent 侧调用用 Node 原生 WebSocket 直连 gateway（与 Console 同一 ABI，无后门）。

import { test, expect, type Page } from "@playwright/test";

const PORT = () => process.env.SCOOTLENS_E2E_PORT ?? "39231";
const ADMIN = () => process.env.SCOOTLENS_E2E_ADMIN ?? "";
const AGENT = () => process.env.SCOOTLENS_E2E_AGENT ?? "";

/** 极简 agent 客户端：一次 JSON-RPC 调用（等待匹配 id 的响应帧）。 */
class AgentWs {
  private ws: WebSocket;
  private seq = 0;
  private ready: Promise<void>;

  constructor(token: string) {
    this.ws = new WebSocket(`ws://127.0.0.1:${PORT()}/ws?token=${encodeURIComponent(token)}`);
    this.ready = new Promise((resolve, reject) => {
      this.ws.addEventListener("open", () => resolve(), { once: true });
      this.ws.addEventListener("error", (e) => reject(e), { once: true });
    });
  }

  async call(method: string, params: unknown): Promise<{ result?: unknown; error?: unknown }> {
    await this.ready;
    const id = ++this.seq;
    const frame = JSON.stringify({ jsonrpc: "2.0", id, method, params });
    return new Promise((resolve) => {
      const onMessage = (ev: MessageEvent) => {
        try {
          const v = JSON.parse(String(ev.data)) as { id?: number };
          if (v.id === id) {
            this.ws.removeEventListener("message", onMessage);
            resolve(v as { result?: unknown; error?: unknown });
          }
        } catch {
          // 忽略非 JSON 帧
        }
      };
      this.ws.addEventListener("message", onMessage);
      this.ws.send(frame);
    });
  }

  close() {
    this.ws.close();
  }
}

async function connectAsAdmin(page: Page): Promise<void> {
  await page.goto(`/?token=${encodeURIComponent(ADMIN())}&connect=1`);
  await expect(page.getByTestId("tab-dashboard")).toBeVisible();
}

async function spawnProc(page: Page): Promise<string> {
  await page.getByTestId("tab-dashboard").click();
  const before = new Set(
    await page.locator("tbody td.mono").allTextContents().then((t) => t.map((s) => s.trim())),
  );
  await page.getByTestId("spawn").click();
  await expect
    .poll(async () => {
      const now = await page.locator("tbody td.mono").allTextContents();
      return now.map((s) => s.trim()).find((p) => !before.has(p)) ?? "";
    })
    .toMatch(/^p-/);
  const now = await page.locator("tbody td.mono").allTextContents();
  return now.map((s) => s.trim()).find((p) => !before.has(p)) ?? "";
}

test("connect + dashboard: engine info and spawn/kill lifecycle", async ({ page }) => {
  await connectAsAdmin(page);
  await expect(page.locator(".stat").first()).toHaveText(/mock/);

  const pid = await spawnProc(page);
  expect(pid).toMatch(/^p-/);
  await expect(page.locator("tbody tr", { hasText: pid })).toContainText("running");

  await page.getByTestId(`kill-${pid}`).click();
  await expect(page.locator("tbody tr", { hasText: pid })).toContainText("terminated");
});

test("session: screencast frames + human takeover holds agent input", async ({ page }) => {
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  // Session 页：选中 pid，导航到 fixtures 登录页
  await page.getByTestId("tab-session").click();
  await page.getByTestId("session-pid").selectOption(pid);
  await page.getByTestId("goto-url").fill("http://fixture.test/login");
  await page.getByRole("button", { name: "导航" }).click();

  // screencast 帧渲染（mock 引擎产出合法 PNG）
  await expect(page.getByTestId("screencast")).toBeVisible();
  // 语义元素清单出现（输入注入面板）
  await expect(page.locator("table", { hasText: "Username" })).toBeVisible();

  // Agent 准备：先证明无接管时输入畅通
  const agent = new AgentWs(AGENT());
  const snap = (await agent.call("view.snapshot", { pid })) as {
    result?: { text?: string };
  };
  const line = (snap.result?.text ?? "")
    .split("\n")
    .find((l) => l.includes('"Username"') && l.includes("["));
  const ref = line?.slice(line.lastIndexOf("[") + 1, line.lastIndexOf("]")) ?? "";
  expect(ref).not.toBe("");
  const free = await agent.call("act.type", { pid, ref, text: "agent-1" });
  expect(free.error).toBeUndefined();

  // 人接管 → Agent 输入被挂起
  await page.getByTestId("takeover").click();
  await expect(page.getByTestId("release")).toBeVisible();

  let heldResolved = false;
  const held = agent
    .call("act.type", { pid, ref, text: "agent-2" })
    .then((r) => {
      heldResolved = true;
      return r;
    });
  await page.waitForTimeout(400);
  expect(heldResolved, "agent input must be held during takeover").toBe(false);

  // 人（holder）注入输入无阻塞：点击第一个元素的 Click
  await page.locator("table tbody tr", { hasText: "Username" }).getByRole("button", { name: "Click" }).click();

  // 归还控制 → Agent 挂起调用恢复并成功
  await page.getByTestId("release").click();
  await expect(page.getByTestId("takeover")).toBeVisible();
  const resumed = await held;
  expect(resumed.error, `held call resumes ok: ${JSON.stringify(resumed)}`).toBeUndefined();

  agent.close();
});

test("approvals: sensitive agent call pends, admin approves, call resumes", async ({ page }) => {
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  const agent = new AgentWs(AGENT());
  await agent.call("nav.goto", { pid, url: "http://fixture.test/" });

  // js.exec 敏感作用域 → 挂起等待人工审批
  let resolved = false;
  const pending = agent.call("js.exec", { pid, script: "1" }).then((r) => {
    resolved = true;
    return r;
  });

  await page.getByTestId("tab-approvals").click();
  const card = page.locator(".approval", { hasText: "agent:e2e" }).first();
  await expect(card).toBeVisible();
  expect(resolved).toBe(false);

  await card.getByRole("button", { name: "批准", exact: true }).click();
  const out = await pending;
  expect(out.error, `approved call resumes: ${JSON.stringify(out)}`).toBeUndefined();

  agent.close();
});

test("journal: entries stream in with verified continuity", async ({ page }) => {
  await connectAsAdmin(page);
  await spawnProc(page);
  await page.getByTestId("tab-journal").click();
  await expect(page.locator(".tag", { hasText: "链完整" })).toBeVisible();
  await expect(page.locator("tbody tr", { hasText: "proc.spawn" }).first()).toBeVisible();
});

test("replay: export bundle, chain verifies, timeline + frame playback", async ({ page }) => {
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  // Session 里产生画面帧（screencast 轮询即 view.screenshot → FrameStore）
  await page.getByTestId("tab-session").click();
  await page.getByTestId("session-pid").selectOption(pid);
  await page.getByTestId("goto-url").fill("http://fixture.test/login");
  await page.getByRole("button", { name: "导航" }).click();
  await expect(page.getByTestId("screencast")).toBeVisible();

  await page.getByTestId("tab-replay").click();
  await page.getByTestId("replay-pid").selectOption(pid);
  await page.getByTestId("replay-export").click();

  await expect(page.getByTestId("chain-status")).toContainText("哈希链完整");
  await expect(page.getByTestId("replay-timeline")).toBeVisible();

  // 帧对齐：时间线开头（spawn 时刻）早于首帧 → 无帧；跳到最后一步 → 帧可见
  await page.getByTestId("replay-timeline").locator("tbody tr").last().click();
  await expect(page.getByTestId("replay-frame")).toBeVisible();

  // 步进控制
  await page.getByRole("button", { name: "← 上一步" }).click();
  await expect(page.locator("tr.active")).toBeVisible();
});

test("settings: token scopes, grant/revoke, vault write-only, net rules", async ({ page }) => {
  await connectAsAdmin(page);
  await page.getByTestId("tab-settings").click();

  // 本会话令牌
  await expect(page.locator(".card", { hasText: "本会话令牌" })).toContainText("user:admin");

  // 动态授权
  await page.getByTestId("grant-subject").fill("agent:e2e");
  await page.getByTestId("grant-scope").fill("nav@docs.fixture.test");
  await page.getByTestId("grant-btn").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已授予");

  // vault 只写不读：写入后仅显示句柄
  await page.getByTestId("vault-name").fill("demo-cred");
  await page.getByTestId("vault-secret").fill("s3cret-value");
  await page.getByTestId("vault-write").click();
  await expect(page.getByTestId("vault-ref")).toContainText("demo-cred");
  await expect(page.getByTestId("vault-secret")).toHaveValue("");

  // 全局网络规则
  await page
    .getByTestId("net-rules")
    .fill('{ "default": "allow", "rules": [ { "action": "deny", "host": "blocked.test" } ] }');
  await page.getByTestId("net-rules-apply").click();
  await expect(page.getByTestId("settings-notice")).toContainText("网络规则已生效");
});
