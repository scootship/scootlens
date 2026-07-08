# 01 · 愿景与定位

## 一句话

**ScootLens 是 Web Operating System**：把 Web 会话当作进程、把站点状态当作文件系统、
把权限做成 capability、把浏览器引擎做成可替换驱动，向 AI Agent 与人类提供统一的系统调用接口。

它不是浏览器代理，不是自动化测试工具，不是 Agent 框架。

## 为什么是 OS 而不是工具

| 工具视角（浏览器代理） | OS 视角（ScootLens） |
|---|---|
| Agent 是中心，浏览器是外设 | 内核是中心，Agent 只是用户空间的一个客户端 |
| 会话即用即弃 | 会话是进程：可挂起、恢复、**快照/还原**、长期驻留 |
| 安全靠 Agent 自觉 | 内核强制执行 capability，与 Agent 判断无关 |
| 单引擎绑定 | 多引擎 HAL：Chromium / WPE / Servo |
| 无审计 | 全链路 journal + trace + 录制回放 |

## 北极星场景

1. **长驻 Agent 运维**：Agent 7×24 驻留多个已登录会话，定时巡检、下单、填报，会话可快照迁移
2. **企业可审计自动化**：每一次点击可追溯、可回放；敏感操作需人工审批；凭据永不进入 LLM 上下文
3. **低资源设备自动化**：WPE 后端在嵌入式/边缘设备上以极低内存运行 Web 自动化

## 竞品对照

| 产品 | 类别 | ScootLens 差异 |
|---|---|---|
| Playwright / Puppeteer | 测试客户端库 | 我们是常驻运行时与平台，不是测试驱动库；无 Node 层 |
| browser-use 等 | Agent 库 | 我们不含 Planner/Memory，是它们可以跑在其上的 OS |
| Browserbase / browserless | 云浏览器 | 我们有 OS 语义：进程快照/恢复、capability、State VFS、多引擎 |
| Chrome MCP / Playwright MCP | MCP 工具 | MCP 只是我们 ABI 的一个投影；内核安全不依赖客户端 |

## 非目标（Non-Goals）

- ❌ 不做面向人类日常浏览的浏览器
- ❌ 不做 LLM 编排框架：Planner、Memory、Prompt 工程属于用户空间，内核永不内置
- ❌ 不做浏览器引擎本身：引擎是"硬件"，我们写驱动
- ❌ v1 前不做多机分布式集群

## 术语表

| 术语 | 含义 |
|---|---|
| Kernel | ScootLens 核心守护进程 `scootlensd` |
| Process / proc | 一个被内核管理的 Web 会话（引擎侧为独立浏览器进程） |
| ABI / Syscall | 客户端与内核之间的系统调用接口 |
| HAL / Driver | 引擎硬件抽象层 / 具体引擎驱动 |
| Capability / cap | 细粒度权限令牌与作用域 |
| State VFS | 按命名空间挂载的状态文件系统（cookie/storage/vault 等） |
| Snapshot | 语义快照：带元素引用的精简可访问性树（区别于进程快照 proc.snapshot） |
| Console | 纯 Web 管理控制台 |
