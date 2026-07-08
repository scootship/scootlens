# ADR-0002：Chromium 接入采用外部进程 CDP 而非 CEF 嵌入

- 状态：Accepted
- 日期：2026-07-08

## 背景

Chromium 后端有两种接入方式：CEF 进程内嵌入，或以独立子进程运行 headless Chromium 并通过
CDP（WebSocket）控制。

## 决策

采用**外部进程 + CDP**。CDP 客户端自研薄封装，类型从 devtools-protocol JSON 代码生成，
仅生成用到的 domain。

## 备选方案

- CEF 嵌入：巨大 C++ FFI 面污染 Rust 安全性；构建复杂；引擎崩溃连累内核；沙箱配置负担；拒绝
- 依赖 chromiumoxide 等现成 crate：维护活跃度不可控，协议面超需求；作为参考实现，不作依赖
- Playwright/WebDriver 驱动：引入 Node 层或额外守护进程，与性能目标矛盾；拒绝

## 后果

- 引擎沙箱完整保留；一 proc 一进程天然隔离；崩溃可恢复（监督 + restore）
- 代价是本机 WebSocket 往返，延迟可忽略（预算 <50ms act 往返）
- 需要管理 chromium 二进制版本锁定（CI 固定版本 + conformance 回归）
