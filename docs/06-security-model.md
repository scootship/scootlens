# 06 · 安全模型

> 本文从**防御者视角**描述系统必须抵御的风险类别。ScootLens 是一个 capability 沙箱内核：
> 下列所有条目都是内核**必须拒绝**的行为，配套测试（`enforcement.rs`）逐条验证拒绝路径生效。

## 原则

1. **内核强制，不信任客户端**：Agent 的"自觉"没有安全价值；一切在 Security Manager + net 层强制
2. **最小权限**：capability 按 主体 × 作用域 × 约束 授予，默认拒绝
3. **凭据永不进入 LLM 上下文**：vault 间接引用是硬性设计
4. **先记后行**：journal 写入先于执行

## Capability 模型

### 令牌

**Claims**（`scootlens-abi::TokenClaims`，签名前的载荷）：

```json
{
  "subject": "agent:ops-bot-1",
  "scopes": ["nav@*.example.com", "view@*.example.com", "act@app.example.com", "vault:use"],
  "constraints": { "expires_at": 1735689600, "rate": "60/min", "approval": {"js:exec@*": "manual"} },
  "issued_by": "user:admin"
}
```

**Wire 格式**（见 [ADR-0007](adr/0007-token-wire-format.md)）：

```
slt1.<base64url(claims_json)>.<base64url(ed25519_sig)>
```

- 前缀 `slt1` 标识版本；两段均为 URL-safe base64（无填充）
- 签名覆盖 claims 字节；内核持有签发密钥（ed25519），握手时验签失败一律 `E_CAP_DENIED`
- `expires_at` 为 unix 秒；`rate` 形如 `N/min` 或 `N/sec`，滑动窗口，超限 `E_QUOTA`
- 令牌经 Gateway **连接握手**提交（不在每条 RPC 帧内），校验通过后连接绑定到解析出的 `Caller`

### 作用域语法

`<domain>:<action>[@<origin-pattern>]`

- `act@*.github.com` —— 在 github.com 任意子域执行输入动作
- `state:read:cookies@github.com` —— 读取指定站点 cookie（🔒）
- `js:exec@localhost:*` —— 仅本地站点执行 JS（🔒）
- `proc:spawn`、`net:rules`、`cap:admin` —— 无 origin 维度的系统级作用域

### 敏感作用域（默认人工审批）

`js:exec`、`state:read/write/export/import`、`act:upload`、`act:takeover`、`net:rules`、
`vault:use`（首次）、`obs:replay`、`cap:admin`

审批流：调用挂起 → Console 弹审批卡（主体/作用域/参数摘要/页面截图）→ 批准（可记忆为规则）/拒绝。

### 态前提门（非作用域）：`act.point.click`

不是所有风险收紧都靠作用域审批。`act.point.click`（接管期间坐标点击，见
[ADR-0010](adr/0010-takeover-point-click.md)）复用普通的 `act@<origin>` 作用域，但内核在
执行前额外校验一条**运行时状态前提**：调用者必须是该 pid **当前**接管的 holder，
否则 `E_CAP_DENIED`——这条检查不属于 capability 模型（不看作用域覆盖，看
`takeover` 状态表），因此没有把它做成新的 sensitive scope（那样会导致接管期间
每次点击都要人工审批，违背"人工接管应当顺滑"的设计目标；holder 身份本身已经在
取得 `act:takeover` 时过了一次审批关卡）。

这条门还有一处刻意的不一致：非 holder 的调用**立即拒绝**，不像其余 `act.*` 那样
经 `takeover_gate` 挂起排队等接管结束后恢复。原因是坐标点击没有 `ref`/generation
过期保护——ref 寻址的动作排队到接管结束后重放仍然安全（页面变了 ref 就会
`E_REF_STALE`），坐标点击排队重放则可能在完全不同的页面状态下盲打一个像素坐标。

## 威胁模型与对策

| # | 风险 | 载体 | 对策 |
|---|---|---|---|
| T1 | **Prompt injection**：页面内容诱导 Agent 发起越权操作 | snapshot/extract 返回值 | 内核策略与 Agent 判断解耦；origin 约束（诱导跳转未授权域 → `nav` 无授权即拒）；敏感作用域人工审批；net denylist |
| T2 | 凭据外泄至 LLM/日志 | act.type、journal | vault 只写不读 + 注入瞬间解引用；journal/trace 全链路脱敏 |
| T3 | 不受信页面内容触发引擎缺陷 | 渲染进程 | 外部进程模式保留引擎完整沙箱；一 proc 一进程一 profile；下载隔离到 `downloads://` 沙箱目录不可执行 |
| T4 | 跨站数据流动：A 站数据被发往 B 站 | nav/act/js | origin 作用域天然隔离；跨 origin 组合操作可配置审批策略；net 层出口规则 |
| T5 | 客户端令牌无效/越权请求 | Gateway 连接 | 令牌签名校验；作用域单点强制；令牌可吊销、限速、限期 |
| T6 | 供应链风险 | 依赖 | `cargo-deny`（advisory/license）、lockfile 锁定、依赖新增需评审 |
| T7 | 审计记录被事后修改 | journal | append-only + 每条哈希链；导出可校验 |

## unsafe 政策

- 除 FFI 驱动 crate 外，所有 crate `#![forbid(unsafe_code)]`
- FFI crate 中每个 `unsafe` 块必须有 `// SAFETY:` 注释说明不变量；PR 需第二人 review
- CI grep 检查 SAFETY 注释缺失

## 秘密管理

- vault 后端：**P2 为 ChaCha20-Poly1305 加密文件**（`<state-dir>/vault/vault.enc`）+ 独立 32 字节密钥文件（`vault.key`，权限 `0600`）；OS keyring（macOS Keychain / Secret Service）为后续演进
- vault **只写不读**：`state.read` 命中 `vault` 命名空间恒返回 `E_CAP_DENIED`；值仅在驱动注入输入（`act.type` 的 `vault_ref`）瞬间由内核解引用
- 出口消毒：对注入过 vault 值的调用，内核对所有 syscall 返回值统一扫描替换，杜绝值经 snapshot/journal/trace 回流（enforcement 测试 T2 零泄漏断言覆盖）
- 仓库零明文密钥：gitleaks 进 CI 门禁
