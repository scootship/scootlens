# 06 · 安全模型

## 原则

1. **内核强制，不信任客户端**：Agent 的"自觉"没有安全价值；一切在 Security Manager + net 层强制
2. **最小权限**：capability 按 主体 × 作用域 × 约束 授予，默认拒绝
3. **凭据永不进入 LLM 上下文**：vault 间接引用是硬性设计
4. **先记后行**：journal 写入先于执行

## Capability 模型

### 令牌

```json
{
  "subject": "agent:ops-bot-1",
  "scopes": ["nav@*.example.com", "view@*.example.com", "act@app.example.com", "vault:use"],
  "constraints": { "expires_at": "...", "rate": "60/min", "approval": {"js:exec@*": "manual"} },
  "issued_by": "user:admin",
  "sig": "ed25519:…"
}
```

### 作用域语法

`<domain>:<action>[@<origin-pattern>]`

- `act@*.github.com` —— 在 github.com 任意子域执行输入动作
- `state:read:cookies@github.com` —— 读取指定站点 cookie（🔒）
- `js:exec@localhost:*` —— 仅本地站点执行 JS（🔒）
- `proc:spawn`、`net:rules`、`cap:admin` —— 无 origin 维度的系统级作用域

### 敏感作用域（默认人工审批）

`js:exec`、`state:read/write/export/import`、`act:upload`、`net:rules`、`vault:use`（首次）、
`obs:replay`、`cap:admin`

审批流：调用挂起 → Console 弹审批卡（主体/作用域/参数摘要/页面截图）→ 批准（可记忆为规则）/拒绝。

## 威胁模型与对策

| # | 威胁 | 载体 | 对策 |
|---|---|---|---|
| T1 | **Prompt injection**：页面内容诱导 Agent 执行恶意操作 | snapshot/extract 返回值 | 内核策略与 Agent 判断解耦；origin 约束（诱导跳转恶意域 → `nav` 无授权即拒）；敏感作用域人工审批；net denylist |
| T2 | 凭据外泄至 LLM/日志 | act.type、journal | vault 只写不读 + 注入瞬间解引用；journal/trace 全链路脱敏 |
| T3 | 恶意页面攻击引擎（0day） | 渲染进程 | 外部进程模式保留引擎完整沙箱；一 proc 一进程一 profile；下载隔离到 `downloads://` 沙箱目录不可执行 |
| T4 | 数据渗出：Agent 把 A 站数据发往 B 站 | nav/act/js | origin 作用域天然隔离；跨 origin 组合操作可配置审批策略；net 层出口规则 |
| T5 | 客户端伪造/越权 | Gateway 连接 | 令牌签名校验；作用域单点强制；令牌可吊销、限速、限期 |
| T6 | 供应链攻击 | 依赖 | `cargo-deny`（advisory/license）、lockfile 锁定、依赖新增需评审 |
| T7 | 审计篡改 | journal | append-only + 每条哈希链；导出可校验 |

## unsafe 政策

- 除 FFI 驱动 crate 外，所有 crate `#![forbid(unsafe_code)]`
- FFI crate 中每个 `unsafe` 块必须有 `// SAFETY:` 注释说明不变量；PR 需第二人 review
- CI grep 检查 SAFETY 注释缺失

## 秘密管理

- vault 后端：OS keyring（macOS Keychain / Secret Service），无 keyring 环境退化为 age 加密文件（密钥由部署者持有）
- 仓库零明文密钥：gitleaks 进 CI 门禁
