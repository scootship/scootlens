# ADR-0008：P3 OS 语义 ABI 面（suspend/snapshot/quota/wf）

- 状态：Accepted
- 日期：2026-02-22
- 关联：[03-abi-spec.md](../03-abi-spec.md)、[04-kernel-design.md](../04-kernel-design.md)、Phase 3

## 背景

P3 把 ScootLens 从"浏览器工具"推进为"进程操作系统"：挂起/恢复、快照/恢复、
资源配额、状态搬运（profile 复用）与后台工作流。需要在 ABI 上定案这些能力的
方法面、作用域与错误语义，并遵守铁律：能力最小授权、journal 先记后行、
敏感数据默认人工审批。

## 决策

### 方法面与作用域

| 方法 | 作用域 | 语义 |
|---|---|---|
| `proc.suspend` / `proc.resume` | `proc:manage` | 挂起释放调度槽（引擎冻结尽力而为，caps.lifecycle 门控）；恢复 FIFO 重排队 |
| `proc.snapshot` | `proc:snapshot` | 导出会话状态+URL，内容寻址落盘，返回 `snap-<16hex>`（sha256 前 8 字节） |
| `proc.restore` | `proc:spawn` | spawn 同 profile → 导航原 URL → 导入状态；引擎不匹配拒绝 |
| `state.export` | `state:export` 🔒 | 导出运行中进程的完整会话状态（敏感：默认 Manual 审批） |
| `state.import` | `state:import` 🔒 | 合并状态到 **profile**（非 pid）；后续以该 profile spawn 时预加载 |
| `wf.create/list/run/cancel` | `wf:manage` | Workflow Daemon 管理面 |
| `proc.spawn` + `quotas` | 超过 `quota_high_bytes` 需追加 `quota:high` | 高配额是特权申请 |

### 关键取舍

- **快照身份 = 内容哈希**（`SnapId = snap-<16hex>`）：同状态幂等得同 id，
  存储天然去重、id 不可预测枚举；代价是"最新快照"语义需上层自己记录。
- **`state.import` 目标是 profile 而不是 pid**：向运行中进程注入状态是会话固定
  攻击面；落到 profile 后由 spawn 预加载，让"身份"成为启动期声明而非运行期变量。
- **配额处置在内核轮询侧**（`quota_poll_interval` 读 `metrics()`），不依赖引擎
  自报超限：驱动只需实现 `metrics`，处置策略（warn/suspend/kill）统一在内核，
  跨引擎语义一致；越过水位去抖（回落后再越界才再次触发）。
- **工作流最小权限**：`wf.create` 校验 `spec.scopes ⊆ 创建者有效作用域`（拒绝
  提权），运行主体是独立的 `wf:<name>`；每步经 Dispatcher 分发，journal 与鉴权
  免费获得，巡检流全程可追溯。
- **cron 用注入时钟**（unix 秒 + 自写 civil-from-days 分解，UTC）：测试可控、
  无第三方 cron 依赖；精度到分钟，30s tick + 按分钟去重。
- **Event Bus 自研背压**：每订阅者独立 VecDeque；高频主题（nav/console/
  net.request）队满丢最旧同类并计数（`dropped` 随下一条事件带给订阅者），
  关键主题（proc.lifecycle/cap.request/quota.exceeded/wf.run）无界永不丢。

## 备选方案

- **tokio broadcast 继续扛背压**：lagged 语义是"整体跳过 N 条"，无法区分
  可丢/不可丢主题，关键事件（审批请求、配额处置）可能静默丢失。放弃。
- **快照存引擎级二进制镜像**（如 Chromium user-data-dir 打包）：保真度高但
  巨大、引擎版本耦合、无法跨引擎恢复。放弃——快照面收敛为 HAL `StateBundle`
  （cookies/storage）+ URL，跨引擎可移植。
- **workflow 用系统 crontab / 外部调度器**：脱离 capability 边界与 journal，
  违反"没有 sudo"铁律。放弃。
- **配额用 cgroup/OS 级限制**：Linux-only 且需要特权；P3 需要的是跨平台的
  策略处置语义而非硬隔离。放弃（硬隔离留给部署层容器化）。

## 后果

- 驱动新增可选 `lifecycle` 能力位（冻结/解冻），mock 与 chromium 均已实现；
  不支持的引擎 suspend 仍生效（仅调度语义，无引擎冻结）。
- `EngineCaps` 新增字段以 `#[serde(default)]` 保持向后兼容。
- 快照文件明文含会话状态：`state_dir` 权限即安全边界（0700，与 vault 同级）。
- conformance 契约放宽：引擎可把 cookie 规范化为富对象（domain/path/…），
  语义值必须往返不变。
