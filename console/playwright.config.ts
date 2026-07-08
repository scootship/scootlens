import { defineConfig } from "@playwright/test";

// Console UI e2e（docs/09-roadmap.md P4 门禁 #4）。
// 全栈拓扑：Playwright(Chromium) → scootlensd(--engine mock, 静态托管 dist) → kernel。
// globalSetup 启动 scootlensd 并把 admin/agent 令牌写入环境。
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/global-setup.ts",
  globalTeardown: "./e2e/global-teardown.ts",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  retries: process.env.CI ? 1 : 0,
  // 共享同一 scootlensd 实例：串行执行保证进程表/审批箱确定性
  workers: 1,
  fullyParallel: false,
  reporter: process.env.CI ? "line" : "list",
  use: {
    baseURL: "http://127.0.0.1:39231",
    trace: "retain-on-failure",
  },
});
