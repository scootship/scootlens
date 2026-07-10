# ADR-0011：导入状态（profiles）与 vault 凭据的可观察与可删除

- 状态：Accepted
- 日期：2026-07-09
- 关联：docs/03-abi-spec.md §state、docs/06-security-model.md、ADR-0008（state.import 落地）

## 背景

`state.import` 把登录会话（cookie/localStorage）并入 `profiles/<name>.json`，同名
profile spawn 时预加载——这是「导入登录态 → 新会话复用」的核心机制。但导入之后
内核没有任何 RPC 能回答三个基本问题：

1. **有哪些 profile？** Console 只能在浏览器 localStorage 里记名字（丢了就成盲区）。
2. **某个 profile 里有什么？** 用户无法核对导入内容（几条 cookie、哪个域、httpOnly 与否）。
3. **怎么删？** 导入的 cookie 是登录凭据；用户换号、退租、误导入后没有清除手段，
   状态在磁盘上无限期存活。

vault 有同样的缺口：凭据只写不读、可列名，但**没有删除**——写错名字、轮换密码、
下线账号后，旧 secret 永久留在加密库里。

同时有一条硬约束：导入的 cookie 值就是会话凭据，等价于 vault 里的密码。任何
「展示」都不能把值送回 Agent/LLM 上下文（威胁模型 T2：凭据外泄至 LLM/日志）。

## 决策

**展示复用既有方法，删除新增一个方法**，全部收敛在 `profiles` 命名空间：

- `state.list {namespace:"profiles"}` → `{names}`——只回 profile 名（与 vault 同形）。
- `state.read {namespace:"profiles", key:<profile>}` → **元数据摘要**：每条 entry 的
  键名、类别（cookie/storage/other）、cookie 的 domain/path/secure/httpOnly、
  值字节数。值本身绝不进入返回（与 vault 只写不读同一原则）。
- `state.delete {namespace:"profiles", key:<profile>, entry?}`——缺省整删，带
  `entry`（如 `cookie:sessionid`）只删单条。只作用于 profile 存储，运行中的
  进程不受影响。profile/entry 不存在 → `E_INVALID_ARG`。
- `state.delete {namespace:"vault", key:<凭据名>}`——删除一条 vault 凭据
  （`entry` 不适用）。删除**不回收**出口脱敏登记：已写入过的 secret 可能仍
  存在于历史 journal 或引擎侧状态，脱敏表保持终身有效。
- `state.write` 对 `profiles` 显式 `E_UNSUPPORTED`（写入必须走 `state.import`，
  保留合并语义与审计口径）。

作用域沿用 `state:<verb>:<ns>` 语法：读摘要要 🔒`state:read:profiles`，删除要
🔒`state:delete:profiles` / 🔒`state:delete:vault`（`state:delete` 进敏感集合——
对存量登录态与凭据的破坏性操作默认人工审批）；`state:list:profiles` 非敏感
（只有名字）。

`ABI_VERSION` 0.3.0 → 0.4.0；方法表 48 → 49。

## 备选方案

- **`state.write` 以 `value: null` 表示删除**：不新增方法，但删除语义藏在写方法里，
  漏传 value 即静默删除，误删面大；且 serde 对 `Option<Value>` 无法区分「缺省」与
  「显式 null」。放弃。
- **摘要返回值哈希而非字节数**：哈希可被离线字典比对（常见 token 前缀），字节数
  已足够核对导入是否完整。放弃。
- **展示也新增方法（如 `state.inspect`）**：`state.read` 的 `<ns>` 维度本就为
  per-namespace 语义分化而设（vault 读即拒、downloads 读即 unsupported），复用
  不增加协议表面。放弃新增。

## 后果

- 正面：导入的登录态与 vault 凭据首次获得完整生命周期（导入/写入 → 核对 →
  复用 → 清除）；Console 不再依赖浏览器 localStorage 记账；误导入/换号/密码轮换
  有一等清除路径。
- 负面：方法表 +1（契约快照、MCP 工具表、no-scope sweep 自动跟随）；`state:delete`
  进敏感集合意味着非 admin 令牌删除需过审批——对「清凭据」这类安全正向操作多一步，
  属可接受的默认保守。
- 迁移：无存量兼容问题（`profiles` 此前对 read/list/delete 均是 unknown namespace
  报 `E_INVALID_ARG`，语义从「不存在」变「已定义」）。
