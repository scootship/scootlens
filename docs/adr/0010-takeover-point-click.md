# ADR-0010：接管期间坐标点击（`act.point.click`）

- 状态：Accepted
- 日期：2026-07-09
- 关联：docs/07-web-console.md、docs/06-security-model.md、docs/03-abi-spec.md、ADR-0009

## 背景

Session 页的人工接管（ADR-0009）只能通过"语义元素清单 → Click/Type 按钮"注入输入：
操作者要在右侧表格里逐行找到目标元素再点按钮，无法直接在左侧 `实时画面` 上点选——
对着一张登录页/验证码/MFA 页面操作时体验很差，也不符合"这就是一块屏幕"的直觉。

参考同作者另一项目 `asuc`（`src/session/remote.ts` 的 `RemoteBrowserView`）：headless 浏览器
经 CDP `Page.startScreencast` 推流画面到网页，鼠标/键盘事件按像素坐标直接经
`Input.dispatchMouseEvent`/`dispatchKeyEvent` 回注——像远程桌面一样直接在画面上操作。
但那是单一人工操作者的免头登录工具，没有 Agent/沙箱边界的概念。

ScootLens 的硬约束（docs/07）：**Console 与 Agent 走同一 ABI，无任何专用后门**。而现有
`act.click/type/press/select/upload` 全部是 `ElementRef` 寻址（`InputAction` 枚举，
`crates/scootlens-hal/src/types.rs`），协议里没有坐标类原语——ref 寻址天然带
generation 过期保护（页面变化后旧 ref 立即失效，`E_REF_STALE`），坐标点击没有。

## 决策

### 1. 新增 ABI 方法 `act.point.click`，Agent 与人共用

- `act.point.click { pid, x_ratio, y_ratio }` → `ActResult`；`x_ratio`/`y_ratio` 是**归一化
  视口坐标 [0,1]**（不是绝对像素）——Console 端用 `offsetX / 内容矩形宽度` 即可算出，
  不需要关心 PNG 原始像素尺寸或 DPR；驱动端按自己当前视口换算成像素
- 复用既有 `act@<origin>` 作用域，**不新增 sensitive scope**：风险面由下面的状态前提收紧，
  而不是逐次人工审批——接管期间每次点击都要审批会直接违背"顺滑交互"这个初衷，也和
  "holder 的普通 act.\* 调用本就直接放行、不重复审批"的既有语义一致
- HAL 新增 `InputAction::ClickAt { x_ratio, y_ratio }`；chromium 驱动复用已有的私有
  `click_at(x, y)`（现有 `Click{target}` 内部也是先用 `DOM.getContentQuads` 算中心点
  再调它，只是这里跳过 ref→中心点解析），按固定视口常量（`VIEWPORT_WIDTH/HEIGHT`，
  与 `process.rs` 的 `--window-size` 同一常量）把比例换算成像素；mock 驱动没有几何模型，
  校验比例范围后语义性 no-op（对齐 `Press`/`Scroll` 现有处理方式）

### 2. 仅当调用者是当前 pid 的接管 holder 时可用；否则立即 `E_CAP_DENIED`

- 内核在 `authz` 通过后、执行前做一次状态前提检查：`takeover_holder(pid) ==
  Some(caller.subject)`，不满足则 `E_CAP_DENIED`（无接管、或接管者是别人，两种情况
  同一错误码，不区分，避免向无权限调用者泄露"谁持有接管"这类信息）
- **故意不经过 `takeover_gate` 挂起队列**——这是与其余 `act.*` 方法唯一的行为分歧，
  必须显式记录原因：ref 寻址的动作被挂起排队后，归还控制时重放仍然安全（ref 过期会
  `E_REF_STALE`，不是悄悄打到错的元素上）；坐标点击没有这层保护，排队到未来某个
  不确定的页面状态下盲打一个像素坐标是不安全的。所以非 holder（含无接管）一律
  立即拒绝，不排队、不重试
- 比例越界或非有限数（`x_ratio`/`y_ratio` 不在 `[0,1]`）→ `E_INVALID_ARG`，这条检查在
  holder 判定之前，与其余方法"先校验参数形状，再判定状态"的顺序一致

### 3. Console：画面直接可点，仅接管中生效

- `<img>` 增加 `onclick`，只在 `view.kind === "held-by-me"` 时转发；纯函数
  `containRect`/`clickRatio`（`console/src/lib/session.ts`）负责把点击偏移换算成
  归一化比例——因为 CSS 用 `object-fit: contain`，画面在容器盒子里可能有 letterbox
  留白，必须先扣掉这部分偏移，否则贴边点击会算错
- 语义元素清单（Click/Type 按钮）保留不变，作为键盘可达的无障碍替代路径——画面点击
  是纯指针交互，没有有意义的键盘等价物

## 备选方案

- **语义快照加坐标、Console 侧 hit-test 出 `ref` 再走原有 `act.click`**：完全不新增 ABI
  面，坐标点击“只是换了种方式选中元素”。架构上更纯粹，但需要给
  `A11yNode`/快照协议加 bounding-rect 字段（改动面更大、影响快照文本这个 LLM 主消费
  路径），且命中精度依赖引擎几何查询的实时性。用户明确选择了"接管期间生效"的坐标
  方案（更贴近远程桌面直觉、实现改动更小），此方案作为**后续可选增强**保留，不冲突
  ——两者可以共存
- **坐标点击标记为 sensitive scope（默认人工审批）**：与"必须持有接管"的状态前提
  重复收紧，且接管本身（`act:takeover`）已经是 sensitive scope，取得 holder 身份已经
  过了一次审批关卡；再对每次点击加审批会让接管期间的顺滑交互体验倒退到"过去每个
  操作都要点批准"，拒绝
- **非 holder 的坐标点击复用 `takeover_gate` 挂起排队**：与其余 `act.*` 一致性更好，
  但坐标点击没有 ref/generation 过期保护，排队到接管结束后再执行可能命中完全不同
  的页面状态，是真实的正确性风险，拒绝（见"决策 2"）

## 后果

- 方法表 47 → 48；`ABI_VERSION` 0.2.0 → 0.3.0（v0 破坏性变更窗口内，新增方法不新增
  sensitive scope）
- 契约 golden（`contract__method_table.snap`）随之更新；`enforcement.rs` 的
  `exhaustive_no_scope_is_denied` 超集参数夹具加入 `x_ratio`/`y_ratio`
- `crates/scootlens-kernel/tests/dispatch_p4.rs` 新增 4 个测试：无接管拒绝、非 holder
  立即拒绝（非挂起）、holder 合法比例放行、越界比例拒绝
- Console 新增 `containRect`/`clickRatio` 纯函数（vitest 覆盖）与 `ConsoleApi.actClickAt`；
  Session 页画面新增可点击态（仅接管中），语义元素面板不变
- 新的错误路径：调用者持有充分 `act@<origin>` 但未持有接管 → `E_CAP_DENIED`（区别于
  作用域完全缺失的 `E_CAP_DENIED`，错误码相同、原因不同，客户端不应假设两者可区分）
