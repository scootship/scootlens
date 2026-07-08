# ADR-0005：Agent 接口采用 原生 ABI + MCP 投影双通道

- 状态：Accepted
- 日期：2026-07-08

## 背景

Agent 生态标准接口是 MCP；但 MCP 面向工具调用，表达不了 ScootLens 全部 OS 语义
（流式事件、审批挂起、令牌治理），且不应让内核 API 被外部协议演进绑架。

## 决策

- **原生 ABI**（JSON-RPC 2.0 over WS）是唯一完整接口，内核只认它
- **MCP server** 是 ABI 的只读投影层（`scootlens-mcp`，基于 rmcp）：工具清单从 ABI 定义
  生成，不含任何独立逻辑与权限判断

## 备选方案

- 只做 MCP：OS 语义表达受限，Console/CLI 也被迫走 MCP；拒绝
- 只做原生 ABI：放弃 Agent 生态即插即用；拒绝

## 后果

- 双通道维护成本靠"投影自动生成"压低；MCP 层零业务逻辑是硬约束
- 权限模型单点在内核，MCP 客户端伪造无法越权
