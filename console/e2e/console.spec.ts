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
  // terminated 默认收起（避免累积霸屏）：展开后可见
  await page.getByTestId("toggle-terminated").click();
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

  // 本会话身份
  await expect(page.locator(".card", { hasText: "本会话身份" })).toContainText("user:admin");

  // 动态授权
  await page.getByTestId("grant-subject").fill("agent:e2e");
  await page.getByTestId("grant-scope").fill("nav@docs.fixture.test");
  await page.getByTestId("grant-btn").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已授予");

  // vault 只写不读：写入后仅显示句柄
  await page.getByTestId("subtab-vault").click();
  await page.getByTestId("vault-name").fill("demo-cred");
  await page.getByTestId("vault-secret").fill("s3cret-value");
  await page.getByTestId("vault-write").click();
  await expect(page.getByTestId("vault-ref")).toContainText("demo-cred");
  await expect(page.getByTestId("vault-secret")).toHaveValue("");

  // 凭据可删除：列表只列名，删除后从列表消失
  await expect(page.getByTestId("vault-table")).toContainText("demo-cred");
  await expect(page.getByTestId("vault-table")).not.toContainText("s3cret-value");
  await page.getByTestId("vault-delete-demo-cred").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已删除凭据「demo-cred」");
  await expect(page.getByTestId("vault-delete-demo-cred")).toHaveCount(0);

  // 短凭据值不限长度：写入成功，且名字不被脱敏误伤（值与名同前缀）
  await page.getByTestId("vault-name").fill("demo-pin");
  await page.getByTestId("vault-secret").fill("demo");
  await page.getByTestId("vault-write").click();
  await expect(page.getByTestId("vault-ref")).toContainText("demo-pin");
  await expect(page.getByTestId("vault-table")).toContainText("demo-pin");
  await page.getByTestId("vault-delete-demo-pin").click();
  await expect(page.getByTestId("vault-delete-demo-pin")).toHaveCount(0);

  // 全局网络规则
  await page.getByTestId("subtab-network").click();
  await page
    .getByTestId("net-rules")
    .fill('{ "default": "allow", "rules": [ { "action": "deny", "host": "blocked.test" } ] }');
  await page.getByTestId("net-rules-apply").click();
  await expect(page.getByTestId("settings-notice")).toContainText("网络规则已生效");
});

test("settings: import login session from pasted cookies → profile reuse", async ({ page }) => {
  await connectAsAdmin(page);
  await page.getByTestId("tab-settings").click();
  await page.getByTestId("subtab-session").click();

  const cookiesJson = JSON.stringify([
    { name: "session", value: "SECRET-httponly", domain: "fixture.test", path: "/", secure: true, httpOnly: true },
    { name: "pref", value: "dark", domain: "fixture.test", path: "/", secure: false, httpOnly: false },
  ]);

  await page.getByTestId("import-profile").fill("gh");
  await page.getByTestId("import-cookies").fill(cookiesJson);
  await page.getByTestId("import-storage").fill('{ "auth": "jwt-abc" }');

  // 粘贴即时预览：2 cookie（httpOnly 1）+ 1 storage
  await expect(page.getByTestId("import-preview")).toContainText("httpOnly 1");

  await page.getByTestId("import-session").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已导入 profile");
  // 成功后清空粘贴区
  await expect(page.getByTestId("import-cookies")).toHaveValue("");

  // 回环校验：以该 profile spawn，导出应带回 httpOnly 会话 cookie。
  const admin = new AgentWs(ADMIN());
  try {
    const sp = await admin.call("proc.spawn", { profile: "gh" });
    const pid = (sp.result as { pid: string }).pid;
    const ex = await admin.call("state.export", { pid });
    const entries = (ex.result as { state: { entries: Record<string, unknown> } }).state.entries;
    expect(entries["cookie:session"]).toMatchObject({ value: "SECRET-httponly", httpOnly: true });
    expect(entries["storage:auth"]).toBe("jwt-abc");
    await admin.call("proc.kill", { pid });
  } finally {
    admin.close();
  }
});

test("settings: imported profiles are inspectable (values redacted) and deletable", async ({ page }) => {
  await connectAsAdmin(page);
  await page.getByTestId("tab-settings").click();
  await page.getByTestId("subtab-session").click();

  // 导入一个带 httpOnly 登录 cookie 的 profile。
  const secret = "TOPSECRET-audit-cookie";
  await page.getByTestId("import-profile").fill("audit");
  await page.getByTestId("import-cookies").fill(
    JSON.stringify([
      { name: "session", value: secret, domain: "fixture.test", path: "/", secure: true, httpOnly: true },
      { name: "pref", value: "compact-mode", domain: "fixture.test", path: "/", secure: false, httpOnly: false },
    ]),
  );
  await page.getByTestId("import-session").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已导入 profile");

  // 内核权威列表里出现（state.list namespace=profiles）。
  await expect(page.getByTestId("profiles-table")).toContainText("audit");

  // 查看摘要：键名/域/标志/字节数可见，cookie 值明文绝不出现在页面上。
  await page.getByTestId("profile-view-audit").click();
  const digest = page.getByTestId("profile-digest");
  await expect(digest).toContainText("cookie:session");
  await expect(digest).toContainText("fixture.test");
  await expect(digest).toContainText("httpOnly");
  await expect(digest).toContainText(`${secret.length} B`);
  await expect(digest).not.toContainText(secret);
  await expect(digest).not.toContainText("compact-mode");

  // 单条删除：session cookie 移除，pref 保留。
  await page.getByTestId("profile-entry-delete-cookie:session").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已从「audit」删除 cookie:session");
  await expect(digest).not.toContainText("cookie:session");
  await expect(digest).toContainText("cookie:pref");

  // 整删：profile 从列表消失。
  await page.getByTestId("profile-delete-audit").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已删除 profile「audit」");
  await expect(page.getByTestId("profile-view-audit")).toHaveCount(0);
});

test("session: matched credential binding fills login fields through vault_ref", async ({ page }) => {
  await connectAsAdmin(page);

  await page.getByTestId("tab-settings").click();
  await page.getByTestId("subtab-vault").click();

  await page.getByTestId("vault-name").fill("fixture-user");
  await page.getByTestId("vault-secret").fill("alice@example.test");
  await page.getByTestId("vault-write").click();
  await expect(page.getByTestId("vault-ref")).toContainText("fixture-user");

  await page.getByTestId("vault-name").fill("fixture-password");
  await page.getByTestId("vault-secret").fill("TOPSECRET-fixture-password");
  await page.getByTestId("vault-write").click();
  await expect(page.getByTestId("vault-ref")).toContainText("fixture-password");

  await page.getByTestId("subtab-bindings").click();
  await page.getByTestId("credential-label").fill("Fixture Login");
  await page.getByTestId("credential-origin").fill("fixture.test");
  await page.getByTestId("credential-username-ref").fill("fixture-user");
  await page.getByTestId("credential-password-ref").fill("fixture-password");
  await page.getByTestId("credential-login-url").fill("http://fixture.test/login");
  await page.getByTestId("credential-save").click();
  await expect(page.getByTestId("settings-notice")).toContainText("凭据绑定已保存");

  const pid = await spawnProc(page);
  await page.getByTestId("tab-session").click();
  await page.getByTestId("session-pid").selectOption(pid);
  await page.getByTestId("goto-url").fill("http://fixture.test/login");
  await page.getByRole("button", { name: "导航" }).click();

  await expect(page.getByTestId("credential-choice")).toContainText("Fixture Login");
  await page.getByTestId("credential-fill").click();

  const table = page.locator("table", { hasText: "Password" });
  await expect(table).toContainText("[REDACTED]");
  await expect(table).not.toContainText("TOPSECRET-fixture-password");
  await expect(table).not.toContainText("alice@example.test");
});

test("session: spawn with imported profile → logged-in session ready for takeover", async ({ page }) => {
  await connectAsAdmin(page);

  // 先导入一个带 httpOnly 会话 cookie 的 profile。
  await page.getByTestId("tab-settings").click();
  await page.getByTestId("subtab-session").click();
  await page.getByTestId("import-profile").fill("reuse");
  await page.getByTestId("import-cookies").fill(
    JSON.stringify([
      { name: "session", value: "LIVE-httponly", domain: "fixture.test", path: "/", secure: true, httpOnly: true },
    ]),
  );
  await page.getByTestId("import-session").click();
  await expect(page.getByTestId("settings-notice")).toContainText("已导入 profile");

  // 到 Session 页：导入过的 profile 会出现在下拉里，选中它 → 「新开会话」。
  await page.getByTestId("tab-session").click();
  await page.getByTestId("spawn-profile").selectOption("reuse");

  const pidSelect = page.getByTestId("session-pid");
  const admin = new AgentWs(ADMIN());
  try {
    // 用权威的 proc.list 做 spawn 前/后差集，精确锁定本次 UI 新开的进程，
    // 不受共享内核里其他测试残留进程的干扰。
    const listPids = async () =>
      ((await admin.call("proc.list")).result as { procs: { pid: string }[] }).procs.map((p) => p.pid);
    const before = new Set(await listPids());

    await page.getByTestId("spawn-with-profile").click();

    let pid = "";
    await expect
      .poll(async () => {
        pid = (await listPids()).find((p) => !before.has(p)) ?? "";
        return pid;
      })
      .toMatch(/^p-/);

    // UI 也应自动选中这个新开的进程（下拉联动），且 running → 接管按钮可用。
    await expect(pidSelect).toHaveValue(pid);
    await expect(page.getByTestId("takeover")).toBeEnabled();

    // 该 console 新开的进程确实预加载了导入的登录态（httpOnly cookie 回灌）。
    const ex = await admin.call("state.export", { pid });
    const entries = (ex.result as { state: { entries: Record<string, unknown> } }).state.entries;
    expect(entries["cookie:session"]).toMatchObject({ value: "LIVE-httponly", httpOnly: true });
    await admin.call("proc.kill", { pid });
  } finally {
    admin.close();
  }
});

test("auth: password login → cookie session drives console; bad password rejected", async ({
  page,
}) => {
  // 无 token 直开 → 登录页（不再要求把令牌贴进 URL）
  await page.goto("/");
  await expect(page.getByTestId("login-user")).toBeVisible();

  // 错误密码 → 明确报错，不进入控制台
  await page.getByTestId("login-user").fill("admin");
  await page.getByTestId("login-pass").fill("wrong-pass");
  await page.getByTestId("login-submit").click();
  await expect(page.locator(".error")).toContainText("用户名或密码错误");
  await expect(page.getByTestId("tab-dashboard")).not.toBeVisible();

  // 正确密码 → cookie 会话 → WS 握手 → Dashboard 可用，身份为 user:admin
  await page.getByTestId("login-pass").fill("e2e-console-pass");
  await page.getByTestId("login-submit").click();
  await expect(page.getByTestId("tab-dashboard")).toBeVisible();
  await expect(page.locator(".sidebar-foot .who")).toContainText("user:admin");

  // 刷新后会话仍在（cookie 持久于本次浏览器上下文）→ 自动重连，无需再登录
  await page.reload();
  await expect(page.getByTestId("tab-dashboard")).toBeVisible();

  // 退出登录 → 回到登录页；再刷新也不会自动进入
  await page.getByRole("button", { name: "退出登录" }).click();
  await expect(page.getByTestId("login-user")).toBeVisible();
  await page.reload();
  await expect(page.getByTestId("login-user")).toBeVisible();
});

test("approvals: checked auto-approve rule resolves pending js.exec without a click", async ({
  page,
}) => {
  // 内核 approval_timeout 默认 60s（人工审批调用内等待上限）；自动批准的
  // round trip（事件送达 → console 自动 approve → 调用恢复）通常 <1s，
  // 但 CI runner 偶发抖动时不应让测试在触达内核自身超时前就被判超时——
  // 用例超时需 ≥ 内核 approval_timeout + 余量，见 KernelConfig::approval_timeout。
  test.setTimeout(75_000);
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  // 勾选 js:exec 自动审批
  await page.getByTestId("tab-approvals").click();
  await page.getByTestId("auto-rule-js:exec").check();

  const agent = new AgentWs(AGENT());
  await agent.call("nav.goto", { pid, url: "http://fixture.test/" });

  // 敏感调用挂起 → cap.request 事件 → console 自动批准 → 调用自行恢复
  const out = await agent.call("js.exec", { pid, script: "40+2" });
  expect(out.error, `auto-approved call resumes: ${JSON.stringify(out)}`).toBeUndefined();
  await expect(page.getByTestId("autoapprove-note")).toContainText("js.exec");

  // 取消勾选后恢复人工审批（挂起不再自动放行）
  await page.getByTestId("auto-rule-js:exec").uncheck();
  let resolved = false;
  const pending = agent.call("js.exec", { pid, script: "1" }).then((r) => {
    resolved = true;
    return r;
  });
  const card = page.locator(".approval", { hasText: "agent:e2e" }).first();
  await expect(card).toBeVisible();
  expect(resolved).toBe(false);
  await card.getByRole("button", { name: "拒绝" }).click();
  const denied = await pending;
  expect(denied.error, "denied call must error").toBeDefined();

  agent.close();
});

test("session: quick kill switches away and terminated pids leave the dropdown", async ({
  page,
}) => {
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  await page.getByTestId("tab-session").click();
  await page.getByTestId("session-pid").selectOption(pid);

  // 快速 Kill → 通知 + 终止的 pid 不再出现在下拉列表
  await page.getByTestId("session-kill").click();
  await expect(page.getByTestId("session-notice")).toContainText(`已终止 ${pid}`);
  const values = await page
    .getByTestId("session-pid")
    .locator("option")
    .evaluateAll((os) => os.map((o) => (o as HTMLOptionElement).value));
  expect(values).not.toContain(pid);
});

test("session: save live session state as a profile for future spawns", async ({ page }) => {
  await connectAsAdmin(page);
  const pid = await spawnProc(page);

  // 会话内产生可导出的登录态（fixture 登录页写 cookie）
  await page.getByTestId("tab-session").click();
  await page.getByTestId("session-pid").selectOption(pid);
  await page.getByTestId("goto-url").fill("http://fixture.test/login");
  await page.getByRole("button", { name: "导航" }).click();
  await expect(page.getByTestId("screencast")).toBeVisible();

  // 保存为 profile → 通知 + spawn 下拉出现该 profile
  await page.getByTestId("save-profile-name").fill("live-save");
  await page.getByTestId("save-profile").click();
  await expect(page.getByTestId("session-notice")).toContainText("live-save");
  const options = await page
    .getByTestId("spawn-profile")
    .locator("option")
    .evaluateAll((os) => os.map((o) => (o as HTMLOptionElement).value));
  expect(options).toContain("live-save");

  // 用它新开会话成功（内核 profile 状态已存在）
  await page.getByTestId("spawn-profile").selectOption("live-save");
  await page.getByTestId("spawn-with-profile").click();
  await expect(page.getByTestId("session-notice")).toContainText("live-save");
});
