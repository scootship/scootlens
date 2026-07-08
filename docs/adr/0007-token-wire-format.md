# ADR-0007：Capability 令牌 wire 格式采用 `slt1.<claims>.<sig>`

- 状态：Accepted
- 日期：2025-02-14
- 关联：[03-abi-spec.md](../03-abi-spec.md)、[06-security-model.md](../06-security-model.md)、Phase 2

## 背景

P2 将 capability 从设计提升为全量强制。Gateway 接受连接时必须校验一枚由内核签发、
完整性受签名保护（第三方无法构造有效令牌）、可离线自证、便于人读排障的令牌。约束：

- 无外部信任方（内核自持签发密钥），不引入 PKI/JWKS 基础设施
- 载荷需承载 `subject / scopes / constraints(expires_at, rate, approval) / issued_by`
- 令牌在纯文本通道（WebSocket 握手、CLI 打印、日志）中传递，需 URL/shell 安全
- 校验必须廉价且恒定失败语义（任何异常→`E_CAP_DENIED`，不泄漏原因）

## 决策

采用自描述三段式：

```
slt1.<base64url(claims_json)>.<base64url(ed25519_sig)>
```

- 前缀 `slt1`（ScootLens Token v1）显式版本化，未来不兼容变更递增前缀
- `claims_json` 为 `scootlens-abi::TokenClaims` 的规范 JSON；类型定义在 abi（协议真源），
  签发/验签在 kernel `security` 模块
- 签名算法 ed25519（`ed25519-dalek`），覆盖 claims 字节；两段均 URL-safe base64 无填充
- 内核启动时 `load_or_generate` 签发密钥至 `<state-dir>`；`scootlensd` 启动打印一枚
  admin 令牌（`user:admin` / `["*"]` / `approval {"*":"auto"}`）用于引导

## 备选方案

- **标准 JWT（JWS）**：生态成熟但引入 header 协商、alg 混淆等额外风险面、库体量大；
  我们无跨信任域互操作需求，自描述前缀更简洁可控。放弃。
- **PASETO**：安全默认好，但仍重于需求，且 v4 的 XChaCha 分支与我们“签名而非加密令牌”
  的诉求不完全对齐（claims 无需保密，只需完整性 + 主体绑定）。放弃。
- **不透明随机令牌 + 服务端会话表**：需有状态存储与吊销表查询；P2 追求无状态可自证，
  且 `expires_at`/`rate`/`approval` 内联于令牌便于审计。放弃（吊销由 grant 表与短期限共同覆盖）。

## 后果

- 正面：零外部依赖的可自证令牌；人读前缀便于排障；版本前缀为演进留出空间；
  claims 类型在 abi 使契约测试（`token_claims` 快照）锁定 wire 表面。
- 负面：claims 明文可读（非机密，但主体/作用域可见）——通道机密性交给 TLS（部署层）；
  吊销依赖短期限 + grant 撤销而非集中黑名单（P3 视需要引入 jti 吊销表）。
- 门禁影响：新增 `token_claims` 契约快照；abi 变更联动 03-abi-spec（门禁#10）；
  新依赖 `ed25519-dalek`(BSD-3) 已在 `deny.toml` 许可清单内。
