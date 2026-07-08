//! syscall 分发层：`RpcRequest` → 鉴权门 → journal → 内核调用 → `RpcResponse`。
//!
//! P2 起每个请求都带已验证的 [`Caller`]，流程（docs/04-kernel-design.md §4.3）：
//!
//! 1. 方法表校验（表外 → JSON-RPC `-32601`）
//! 2. journal 记 `call`（参数已脱敏）——先记后行
//! 3. 参数解析（serde 强校验，失败 → `E_INVALID_ARG`）
//! 4. 鉴权：作用域覆盖 → 限速 → 审批（manual → 调用内挂起等待）
//! 5. 执行；返回值统一出口消毒（vault 值零泄漏）
//! 6. journal 记 `result` / `deny`
//!
//! 方法 → 作用域映射见 docs/06-security-model.md；未落地方法先鉴权后
//! 返回 `E_UNSUPPORTED`（穷举门禁：无令牌/越权一律 `E_CAP_DENIED`）。

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use scootlens_abi::{
    AbiError, ApprovalDecision, ErrorCode, NetDefault, NetRule, NetRuleSet, Pid, QuotaSpec,
    RpcError, RpcId, RpcOutcome, RpcRequest, RpcResponse, Scope, SnapId, WfSpec, method,
};
use scootlens_hal::{A11yNode, HistoryDir, InputAction, ProfileSpec, SnapshotOpts, StateBundle};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::bus::BusPayload;
use crate::journal::JournalKind;
use crate::security::{AuthzGate, Caller};
use crate::{Kernel, origin_of};

/// syscall 分发器。廉价 Clone。
#[derive(Clone)]
pub struct Dispatcher {
    kernel: Kernel,
    wf: std::sync::Arc<crate::wf::WfDaemon>,
}

impl Dispatcher {
    pub fn new(kernel: Kernel) -> Self {
        Self::with_wf_clock(kernel, crate::wf::system_clock())
    }

    /// 注入 workflow 时钟（unix 秒）——cron 触发的确定性测试用。
    pub fn with_wf_clock(kernel: Kernel, clock: crate::WfClock) -> Self {
        Self {
            kernel,
            wf: std::sync::Arc::new(crate::wf::WfDaemon::new(clock)),
        }
    }

    /// 底层内核（gateway 订阅事件用）。
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// 分发一个请求。任何错误都折叠进 `RpcResponse`，本函数不失败。
    pub async fn dispatch(&self, caller: &Caller, req: RpcRequest) -> RpcResponse {
        let id = req.id.clone();
        if !method::is_known(&req.method) {
            return method_not_found(id, &req.method);
        }
        let journal = self.kernel.journal();
        let redactor = self.kernel.redactor();
        let pid_str = req
            .params
            .get("pid")
            .and_then(Value::as_str)
            .map(str::to_owned);

        // 预登记：vault 写入的秘密必须在落 journal 前进入脱敏表，
        // 否则首次写入的明文会随 `call` 记录泄漏（journal 在 route 之前）。
        if req.method == method::STATE_WRITE
            && req.params.get("namespace").and_then(Value::as_str) == Some("vault")
            && let Some(secret) = req.params.get("value").and_then(Value::as_str)
        {
            redactor.add(secret);
        }

        let mut params_summary = req.params.clone();
        redactor.sanitize(&mut params_summary);
        journal.record(
            JournalKind::Call,
            &caller.subject,
            &req.method,
            pid_str.as_deref(),
            json!({ "params": params_summary }),
        );

        match self.route(caller, &req.method, req.params).await {
            Ok(mut result) => {
                redactor.sanitize(&mut result);
                journal.record(
                    JournalKind::Result,
                    &caller.subject,
                    &req.method,
                    pid_str.as_deref(),
                    json!({ "ok": true }),
                );
                RpcResponse::success(id, result)
            }
            Err(e) => {
                let kind = match e.code {
                    ErrorCode::CapDenied | ErrorCode::Quota | ErrorCode::ApprovalPending => {
                        JournalKind::Deny
                    }
                    _ => JournalKind::Result,
                };
                journal.record(
                    kind,
                    &caller.subject,
                    &req.method,
                    pid_str.as_deref(),
                    json!({ "ok": false, "code": e.code.as_str() }),
                );
                RpcResponse::failure(id, e)
            }
        }
    }

    /// 鉴权门。`manual` 审批 → 发 `cap.request` 事件并调用内挂起等待。
    async fn authz(&self, caller: &Caller, required: Scope, m: &str) -> Result<(), AbiError> {
        let gate = self
            .kernel
            .security()
            .check(caller, &required, m, json!({}))?;
        match gate {
            AuthzGate::Allowed => Ok(()),
            AuthzGate::NeedsApproval { pending, rx } => {
                self.kernel.emit(
                    None,
                    BusPayload::CapRequest {
                        approval_id: pending.id.clone(),
                        method: m.to_owned(),
                        scope: pending.scope.to_string(),
                    },
                );
                match tokio::time::timeout(self.kernel.approval_timeout(), rx).await {
                    Ok(Ok((ApprovalDecision::Allow, _))) => Ok(()),
                    Ok(Ok((ApprovalDecision::Deny, _))) => Err(AbiError::new(
                        ErrorCode::CapDenied,
                        format!("approval {} denied", pending.id),
                    )),
                    Ok(Err(_)) => Err(AbiError::new(
                        ErrorCode::Internal,
                        "approval channel closed",
                    )),
                    Err(_) => Err(AbiError::new(
                        ErrorCode::ApprovalPending,
                        format!("approval {} still pending", pending.id),
                    )),
                }
            }
        }
    }

    /// 携带当前页 origin 的要求态作用域。
    fn scope_at(&self, segs: &[&str], pid: &Pid) -> Scope {
        let origin = self.kernel.current_origin(pid);
        Scope::required(segs, origin.as_deref())
    }

    #[allow(clippy::too_many_lines)]
    async fn route(&self, caller: &Caller, m: &str, params: Value) -> Result<Value, AbiError> {
        let k = &self.kernel;
        match m {
            // ---------- proc ----------
            method::PROC_SPAWN => {
                let p: SpawnParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "spawn"], None), m)
                    .await?;
                // 高配额需要额外的 quota:high（docs/04 §4.2）
                if let Some(q) = &p.quotas
                    && q.max_memory_bytes > k.quota_high_bytes()
                {
                    self.authz(caller, Scope::required(&["quota", "high"], None), m)
                        .await?;
                }
                let profile = ProfileSpec {
                    name: p.profile.unwrap_or_else(|| "default".into()),
                    download_dir: None,
                };
                let pid = k.spawn_with(profile, p.quotas).await?;
                Ok(json!({ "pid": pid }))
            }
            method::PROC_LIST => {
                let _: Empty = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "list"], None), m)
                    .await?;
                Ok(json!({ "procs": k.list().await }))
            }
            method::PROC_INFO => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "list"], None), m)
                    .await?;
                Ok(to_value(k.info(&parse_pid(&p.pid)?).await?))
            }
            method::PROC_KILL => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "kill"], None), m)
                    .await?;
                k.kill(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "ok": true }))
            }
            method::PROC_SUSPEND => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "manage"], None), m)
                    .await?;
                k.suspend(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "ok": true }))
            }
            method::PROC_RESUME => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "manage"], None), m)
                    .await?;
                k.resume(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "ok": true }))
            }
            method::PROC_SNAPSHOT => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "snapshot"], None), m)
                    .await?;
                let snap = k.snapshot_proc(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "snap_id": snap }))
            }
            method::PROC_RESTORE => {
                let p: RestoreParams = parse(params)?;
                self.authz(caller, Scope::required(&["proc", "spawn"], None), m)
                    .await?;
                let snap: SnapId = p
                    .snap_id
                    .parse()
                    .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))?;
                let pid = k.restore_proc(&snap, p.engine.as_deref()).await?;
                Ok(json!({ "pid": pid }))
            }
            // ---------- nav ----------
            method::NAV_GOTO => {
                let p: GotoParams = parse(params)?;
                let url = parse_url(&p.url)?;
                let origin = origin_of(&url);
                self.authz(caller, Scope::required(&["nav"], origin.as_deref()), m)
                    .await?;
                Ok(to_value(k.navigate(&parse_pid(&p.pid)?, &url).await?))
            }
            method::NAV_BACK => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["nav"], &pid), m).await?;
                Ok(to_value(k.history(&pid, HistoryDir::Back).await?))
            }
            method::NAV_FORWARD => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["nav"], &pid), m).await?;
                Ok(to_value(k.history(&pid, HistoryDir::Forward).await?))
            }
            method::NAV_RELOAD => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["nav"], &pid), m).await?;
                Ok(to_value(k.reload(&pid).await?))
            }
            // ---------- view / dom ----------
            method::VIEW_SNAPSHOT => {
                let p: SnapshotParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["view"], &pid), m)
                    .await?;
                let opts = SnapshotOpts {
                    max_nodes: p
                        .max_nodes
                        .unwrap_or_else(|| SnapshotOpts::default().max_nodes),
                };
                let snap = k.snapshot(&pid, &opts).await?;
                Ok(json!({
                    "generation": snap.generation,
                    "truncated": snap.truncated,
                    "text": snap.to_compact_text(),
                }))
            }
            method::VIEW_SCREENSHOT => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["view"], &pid), m)
                    .await?;
                let bytes = k.screenshot(&pid).await?;
                Ok(json!({ "format": "png", "data_base64": BASE64.encode(bytes) }))
            }
            method::DOM_EXTRACT => {
                let p: ExtractParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["view"], &pid), m)
                    .await?;
                let snap = k.snapshot(&pid, &SnapshotOpts::default()).await?;
                let mut nodes = Vec::new();
                collect_nodes(&snap.root, p.role.as_deref(), p.name.as_deref(), &mut nodes);
                nodes.truncate(p.max.unwrap_or(100));
                Ok(json!({ "generation": snap.generation, "nodes": nodes }))
            }
            // ---------- act ----------
            method::ACT_CLICK => {
                let p: RefParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act"], &pid), m).await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let action = InputAction::Click {
                    target: parse_ref(&p.r#ref)?,
                };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            method::ACT_TYPE => {
                let p: TypeParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act"], &pid), m).await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let text = match (&p.vault_ref, &p.text) {
                    (Some(name), text) => {
                        if text.as_deref().is_some_and(|t| !t.is_empty()) {
                            return Err(AbiError::new(
                                ErrorCode::InvalidArg,
                                "text and vault_ref are mutually exclusive",
                            ));
                        }
                        self.authz(caller, self.scope_at(&["vault", "use"], &pid), m)
                            .await?;
                        let secret = k.vfs().vault_resolve(name).ok_or_else(|| {
                            AbiError::new(ErrorCode::InvalidArg, format!("no vault entry {name:?}"))
                        })?;
                        k.redactor().add(&secret);
                        secret
                    }
                    (None, Some(t)) => t.clone(),
                    (None, None) => {
                        return Err(AbiError::new(
                            ErrorCode::InvalidArg,
                            "either text or vault_ref is required",
                        ));
                    }
                };
                let action = InputAction::Type {
                    target: parse_ref(&p.r#ref)?,
                    text,
                };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            method::ACT_PRESS => {
                let p: PressParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act"], &pid), m).await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let action = InputAction::Press { keys: p.keys };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            method::ACT_SCROLL => {
                let p: ScrollParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act"], &pid), m).await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let target = match &p.r#ref {
                    Some(r) => Some(parse_ref(r)?),
                    None => None,
                };
                let action = InputAction::Scroll {
                    target,
                    dx: p.dx,
                    dy: p.dy,
                };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            method::ACT_SELECT => {
                let p: SelectParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act"], &pid), m).await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let action = InputAction::Select {
                    target: parse_ref(&p.r#ref)?,
                    values: p.values,
                };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            method::ACT_UPLOAD => {
                let p: UploadParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["act", "upload"], &pid), m)
                    .await?;
                k.takeover_gate(&pid, &caller.subject).await?;
                let resolved = k.vfs().resolve_upload(&p.path)?;
                let action = InputAction::Upload {
                    target: parse_ref(&p.r#ref)?,
                    paths: vec![resolved],
                };
                Ok(to_value(k.dispatch(&pid, &action).await?))
            }
            // ---------- takeover（P4：人工接管，docs/07-web-console.md） ----------
            method::ACT_TAKEOVER_START => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, Scope::required(&["act", "takeover"], None), m)
                    .await?;
                k.takeover_start(&pid, &caller.subject)?;
                Ok(json!({ "ok": true, "holder": caller.subject }))
            }
            method::ACT_TAKEOVER_END => {
                let p: PidParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, Scope::required(&["act", "takeover"], None), m)
                    .await?;
                k.takeover_end(&pid, &caller.subject)?;
                Ok(json!({ "ok": true }))
            }
            // ---------- js ----------
            method::JS_EXEC => {
                let p: EvalParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["js", "exec"], &pid), m)
                    .await?;
                let out = k.eval(&pid, &p.script, &p.args.unwrap_or_default()).await?;
                Ok(json!({ "value": out }))
            }
            // ---------- evt ----------
            method::EVT_WAIT => {
                let p: WaitParams = parse(params)?;
                let pid = parse_pid(&p.pid)?;
                self.authz(caller, self.scope_at(&["view"], &pid), m)
                    .await?;
                self.wait_event(pid, p.cond, p.timeout_ms).await
            }
            // ---------- state ----------
            method::STATE_READ => {
                let p: StateParams = parse(params)?;
                self.authz(
                    caller,
                    Scope::required(&["state", "read", &p.namespace], None),
                    m,
                )
                .await?;
                self.state_read(&p).await
            }
            method::STATE_WRITE => {
                let p: StateParams = parse(params)?;
                self.authz(
                    caller,
                    Scope::required(&["state", "write", &p.namespace], None),
                    m,
                )
                .await?;
                self.state_write(&p).await
            }
            method::STATE_LIST => {
                let p: StateParams = parse(params)?;
                self.authz(
                    caller,
                    Scope::required(&["state", "list", &p.namespace], None),
                    m,
                )
                .await?;
                self.state_list(&p).await
            }
            method::STATE_EXPORT => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["state", "export"], None), m)
                    .await?;
                let bundle = k.export_state(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "state": bundle }))
            }
            method::STATE_IMPORT => {
                let p: StateImportParams = parse(params)?;
                self.authz(caller, Scope::required(&["state", "import"], None), m)
                    .await?;
                k.import_profile_state(&p.profile, &p.state)?;
                Ok(json!({ "ok": true }))
            }
            // ---------- net ----------
            method::NET_RULES_SET => {
                let p: NetRulesSetParams = parse(params)?;
                self.authz(caller, Scope::required(&["net", "rules"], None), m)
                    .await?;
                let pid = p.pid.as_deref().map(parse_pid).transpose()?;
                let rules = NetRuleSet {
                    default: p.default.unwrap_or_default(),
                    rules: p.rules.unwrap_or_default(),
                };
                k.netstack().set_rules(pid.as_ref(), rules);
                Ok(json!({ "ok": true }))
            }
            method::NET_RULES_GET => {
                let p: NetPidParams = parse(params)?;
                self.authz(caller, Scope::required(&["net", "rules", "read"], None), m)
                    .await?;
                let pid = p.pid.as_deref().map(parse_pid).transpose()?;
                Ok(json!({ "rules": k.netstack().get_rules(pid.as_ref()) }))
            }
            method::NET_LOG => {
                let p: NetLogParams = parse(params)?;
                self.authz(caller, Scope::required(&["net", "log"], None), m)
                    .await?;
                let pid = p.pid.as_deref().map(parse_pid).transpose()?;
                let entries = k.netstack().log(pid.as_ref(), p.limit.unwrap_or(100));
                Ok(json!({ "entries": entries }))
            }
            // ---------- cap ----------
            method::CAP_REQUEST => {
                let p: CapRequestParams = parse(params)?;
                let scope: Scope = p
                    .scope
                    .parse()
                    .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))?;
                let (pending, _rx) = k.security().begin_approval(
                    &caller.subject,
                    scope.clone(),
                    m,
                    json!({}),
                    p.reason,
                );
                k.emit(
                    None,
                    BusPayload::CapRequest {
                        approval_id: pending.id.clone(),
                        method: m.to_owned(),
                        scope: scope.to_string(),
                    },
                );
                Ok(json!({ "approval_id": pending.id }))
            }
            method::CAP_LIST => {
                let _: Empty = parse(params)?;
                let scopes: Vec<String> = k
                    .security()
                    .effective_scopes(caller)
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                Ok(json!({ "subject": caller.subject, "scopes": scopes }))
            }
            method::CAP_GRANT => {
                let p: CapGrantParams = parse(params)?;
                self.authz(caller, Scope::required(&["cap", "admin"], None), m)
                    .await?;
                let scope: Scope = p
                    .scope
                    .parse()
                    .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))?;
                k.security().grant(&p.subject, scope);
                Ok(json!({ "ok": true }))
            }
            method::CAP_REVOKE => {
                let p: CapGrantParams = parse(params)?;
                self.authz(caller, Scope::required(&["cap", "admin"], None), m)
                    .await?;
                let scope: Scope = p
                    .scope
                    .parse()
                    .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))?;
                k.security().revoke(&p.subject, scope);
                Ok(json!({ "ok": true }))
            }
            method::CAP_APPROVE => {
                let p: CapApproveParams = parse(params)?;
                self.authz(caller, Scope::required(&["cap", "admin"], None), m)
                    .await?;
                let info = k.security().approve(
                    &p.approval_id,
                    p.decision,
                    p.remember.unwrap_or(false),
                )?;
                k.journal().record(
                    JournalKind::Approval,
                    &caller.subject,
                    m,
                    None,
                    json!({
                        "approval_id": p.approval_id,
                        "decision": to_value(p.decision),
                        "subject": info.subject,
                        "scope": info.scope.to_string(),
                    }),
                );
                Ok(json!({ "ok": true, "approval": to_value(info) }))
            }
            method::CAP_PENDING => {
                let _: Empty = parse(params)?;
                self.authz(caller, Scope::required(&["cap", "admin"], None), m)
                    .await?;
                Ok(json!({ "pending": k.security().pending_list() }))
            }
            // ---------- obs ----------
            method::OBS_JOURNAL => {
                let p: ObsParams = parse(params)?;
                self.authz(caller, Scope::required(&["obs", "journal"], None), m)
                    .await?;
                let entries = k.journal().tail(p.limit.unwrap_or(100), p.pid.as_deref());
                Ok(json!({ "entries": entries }))
            }
            method::OBS_TRACE => {
                let p: PidParams = parse(params)?;
                self.authz(caller, Scope::required(&["obs", "trace"], None), m)
                    .await?;
                let entries = k.journal().tail(200, Some(&p.pid));
                Ok(json!({ "pid": p.pid, "entries": entries }))
            }
            method::OBS_REPLAY_EXPORT => {
                let p: ReplayParams = parse(params)?;
                self.authz(caller, Scope::required(&["obs", "replay"], None), m)
                    .await?;
                let bundle =
                    k.replay_export(&parse_pid(&p.pid)?, p.journal_limit.unwrap_or(1000))?;
                Ok(json!({ "bundle": bundle }))
            }
            // ---------- wf ----------
            method::WF_CREATE => {
                let p: WfCreateParams = parse(params)?;
                self.authz(caller, Scope::required(&["wf", "manage"], None), m)
                    .await?;
                self.wf.create(self, caller, p.spec)?;
                Ok(json!({ "ok": true }))
            }
            method::WF_LIST => {
                let _: Empty = parse(params)?;
                self.authz(caller, Scope::required(&["wf", "manage"], None), m)
                    .await?;
                Ok(self.wf.list())
            }
            method::WF_RUN => {
                let p: WfNameParams = parse(params)?;
                self.authz(caller, Scope::required(&["wf", "manage"], None), m)
                    .await?;
                self.wf.run(self, &p.name).await
            }
            method::WF_CANCEL => {
                let p: WfNameParams = parse(params)?;
                self.authz(caller, Scope::required(&["wf", "manage"], None), m)
                    .await?;
                self.wf.cancel(&p.name)?;
                Ok(json!({ "ok": true }))
            }
            // ---------- sys ----------
            method::SYS_INFO => {
                let _: Empty = parse(params)?;
                Ok(to_value(k.sys_info().await))
            }
            method::EVT_SUBSCRIBE | method::EVT_UNSUBSCRIBE => Err(AbiError::new(
                ErrorCode::Unsupported,
                format!("{m} is connection-scoped; use the gateway session"),
            )),
            // ---------- 未落地（先鉴权，穷举门禁） ----------
            other => {
                if let Some(scope) = fallback_scope(other) {
                    self.authz(caller, scope, other).await?;
                }
                Err(AbiError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not implemented in this phase"),
                ))
            }
        }
    }

    // ---------- state 实现 ----------

    async fn state_read(&self, p: &StateParams) -> Result<Value, AbiError> {
        let k = &self.kernel;
        match p.namespace.as_str() {
            // vault 对 Agent 永远只写不读
            "vault" => Err(AbiError::new(
                ErrorCode::CapDenied,
                "vault is write-only for agents",
            )),
            "downloads" => Err(AbiError::new(
                ErrorCode::Unsupported,
                "downloads content read lands in P3 (use state.list)",
            )),
            ns @ ("cookies" | "storage") => {
                let pid = require_pid(p)?;
                let bundle = k.export_state(&pid).await?;
                let prefix = ns_prefix(ns);
                match &p.key {
                    Some(key) => {
                        let full = format!("{prefix}{key}");
                        let value = bundle.entries.get(&full).cloned();
                        Ok(json!({ "value": value }))
                    }
                    None => {
                        let entries: serde_json::Map<String, Value> = bundle
                            .entries
                            .iter()
                            .filter(|(k, _)| k.starts_with(prefix))
                            .map(|(k, v)| (k[prefix.len()..].to_owned(), v.clone()))
                            .collect();
                        Ok(json!({ "entries": entries }))
                    }
                }
            }
            other => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("unknown namespace {other:?}"),
            )),
        }
    }

    async fn state_write(&self, p: &StateParams) -> Result<Value, AbiError> {
        let k = &self.kernel;
        let key = p
            .key
            .as_deref()
            .ok_or_else(|| AbiError::new(ErrorCode::InvalidArg, "key is required"))?;
        let value = p
            .value
            .clone()
            .ok_or_else(|| AbiError::new(ErrorCode::InvalidArg, "value is required"))?;
        match p.namespace.as_str() {
            "vault" => {
                let secret = value.as_str().ok_or_else(|| {
                    AbiError::new(ErrorCode::InvalidArg, "vault value must be a string")
                })?;
                k.vfs().vault_write(key, secret)?;
                // 写入即登记出口消毒
                k.redactor().add(secret);
                Ok(json!({ "ok": true }))
            }
            ns @ ("cookies" | "storage") => {
                let pid = require_pid(p)?;
                let mut bundle = StateBundle::default();
                bundle
                    .entries
                    .insert(format!("{}{key}", ns_prefix(ns)), value);
                k.import_state(&pid, &bundle).await?;
                Ok(json!({ "ok": true }))
            }
            "downloads" => Err(AbiError::new(
                ErrorCode::Unsupported,
                "downloads namespace is engine-written only",
            )),
            other => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("unknown namespace {other:?}"),
            )),
        }
    }

    async fn state_list(&self, p: &StateParams) -> Result<Value, AbiError> {
        let k = &self.kernel;
        match p.namespace.as_str() {
            "vault" => Ok(json!({ "names": k.vfs().vault_names() })),
            "downloads" => Ok(json!({ "names": k.vfs().list_files("downloads")? })),
            ns @ ("cookies" | "storage") => {
                let pid = require_pid(p)?;
                let bundle = k.export_state(&pid).await?;
                let prefix = ns_prefix(ns);
                let names: Vec<String> = bundle
                    .entries
                    .keys()
                    .filter(|k| k.starts_with(prefix))
                    .map(|k| k[prefix.len()..].to_owned())
                    .collect();
                Ok(json!({ "names": names }))
            }
            other => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("unknown namespace {other:?}"),
            )),
        }
    }

    async fn wait_event(
        &self,
        pid: Pid,
        cond: WaitCond,
        timeout_ms: u64,
    ) -> Result<Value, AbiError> {
        let mut rx = self.kernel.subscribe();
        let deadline = Duration::from_millis(timeout_ms);
        let fut = async {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        if e.pid.as_ref() != Some(&pid) {
                            continue;
                        }
                        if cond.matches(&e.payload) {
                            return Ok(e);
                        }
                    }
                    Err(crate::bus::BusRecvError::Closed) => {
                        return Err(AbiError::new(ErrorCode::Internal, "event bus closed"));
                    }
                }
            }
        };
        match tokio::time::timeout(deadline, fut).await {
            Ok(Ok(e)) => Ok(json!({ "event": e })),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(AbiError::new(
                ErrorCode::Timeout,
                format!("no matching event within {timeout_ms}ms"),
            )),
        }
    }
}

/// 未落地方法的规划作用域（先鉴权再 `E_UNSUPPORTED`）。P4 起方法表全部落地，
/// 本表留空备后续阶段新方法使用。
fn fallback_scope(m: &str) -> Option<Scope> {
    let _ = m;
    None
}

fn ns_prefix(ns: &str) -> &'static str {
    match ns {
        "cookies" => "cookie:",
        _ => "storage:",
    }
}

fn require_pid(p: &StateParams) -> Result<Pid, AbiError> {
    let pid = p.pid.as_deref().ok_or_else(|| {
        AbiError::new(
            ErrorCode::InvalidArg,
            format!("namespace {:?} requires pid", p.namespace),
        )
    })?;
    parse_pid(pid)
}

/// DFS 收集匹配节点（dom.extract）。
fn collect_nodes(n: &A11yNode, role: Option<&str>, name: Option<&str>, out: &mut Vec<Value>) {
    let role_ok = role.is_none_or(|r| n.role == r);
    let name_ok = name.is_none_or(|q| n.name.contains(q));
    if role_ok && name_ok {
        out.push(json!({
            "role": n.role,
            "name": n.name,
            "value": n.value,
            "ref": n.elem_ref.as_ref().map(std::string::ToString::to_string),
        }));
    }
    for c in &n.children {
        collect_nodes(c, role, name, out);
    }
}

// ---------- 参数类型 ----------

#[derive(Deserialize)]
struct Empty {}

#[derive(Deserialize)]
struct SpawnParams {
    profile: Option<String>,
    /// 资源配额；高于内核 `quota_high_bytes` 时需 `quota:high`。
    #[serde(default)]
    quotas: Option<QuotaSpec>,
}

#[derive(Deserialize)]
struct RestoreParams {
    snap_id: String,
    #[serde(default)]
    engine: Option<String>,
}

#[derive(Deserialize)]
struct StateImportParams {
    /// 导入目标是 profile（复用机制），不是运行中的 pid。
    profile: String,
    state: StateBundle,
}

#[derive(Deserialize)]
struct WfCreateParams {
    spec: WfSpec,
}

#[derive(Deserialize)]
struct WfNameParams {
    name: String,
}

#[derive(Deserialize)]
struct PidParams {
    pid: String,
}

#[derive(Deserialize)]
struct GotoParams {
    pid: String,
    url: String,
}

#[derive(Deserialize)]
struct SnapshotParams {
    pid: String,
    max_nodes: Option<usize>,
}

#[derive(Deserialize)]
struct ExtractParams {
    pid: String,
    role: Option<String>,
    name: Option<String>,
    max: Option<usize>,
}

#[derive(Deserialize)]
struct RefParams {
    pid: String,
    r#ref: String,
}

#[derive(Deserialize)]
struct TypeParams {
    pid: String,
    r#ref: String,
    #[serde(default)]
    text: Option<String>,
    /// `vault://` 注入：值为 vault 条目名；与 text 互斥。
    #[serde(default)]
    vault_ref: Option<String>,
}

#[derive(Deserialize)]
struct PressParams {
    pid: String,
    keys: String,
}

#[derive(Deserialize)]
struct ScrollParams {
    pid: String,
    r#ref: Option<String>,
    #[serde(default)]
    dx: f64,
    #[serde(default)]
    dy: f64,
}

#[derive(Deserialize)]
struct SelectParams {
    pid: String,
    r#ref: String,
    values: Vec<String>,
}

#[derive(Deserialize)]
struct UploadParams {
    pid: String,
    r#ref: String,
    /// `uploads/` 沙箱内的相对路径。
    path: String,
}

#[derive(Deserialize)]
struct EvalParams {
    pid: String,
    script: String,
    args: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct WaitParams {
    pid: String,
    cond: WaitCond,
    timeout_ms: u64,
}

#[derive(Deserialize)]
struct StateParams {
    #[serde(default)]
    pid: Option<String>,
    namespace: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    value: Option<Value>,
}

#[derive(Deserialize)]
struct NetRulesSetParams {
    #[serde(default)]
    pid: Option<String>,
    #[serde(default)]
    default: Option<NetDefault>,
    #[serde(default)]
    rules: Option<Vec<NetRule>>,
}

#[derive(Deserialize)]
struct NetPidParams {
    #[serde(default)]
    pid: Option<String>,
}

#[derive(Deserialize)]
struct NetLogParams {
    #[serde(default)]
    pid: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct CapRequestParams {
    scope: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
struct CapGrantParams {
    subject: String,
    scope: String,
}

#[derive(Deserialize)]
struct CapApproveParams {
    approval_id: String,
    decision: ApprovalDecision,
    #[serde(default)]
    remember: Option<bool>,
}

#[derive(Deserialize)]
struct ObsParams {
    #[serde(default)]
    pid: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct ReplayParams {
    pid: String,
    /// journal 链段行数上限（默认 1000，内核夹取 1..=4096）。
    #[serde(default)]
    journal_limit: Option<usize>,
}

/// evt.wait 条件（P1 最小集）。
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaitCond {
    UrlContains(String),
    Lifecycle(crate::ProcState),
}

impl WaitCond {
    fn matches(&self, payload: &BusPayload) -> bool {
        match (self, payload) {
            (WaitCond::UrlContains(sub), BusPayload::Navigated { url }) => {
                url.as_str().contains(sub.as_str())
            }
            (WaitCond::Lifecycle(want), BusPayload::ProcLifecycle { state }) => state == want,
            _ => false,
        }
    }
}

// ---------- 辅助 ----------

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AbiError> {
    // 无参调用允许省略 params（Null 视为空对象）
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params)
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("invalid params: {e}")))
}

fn parse_pid(s: &str) -> Result<Pid, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))
}

fn parse_ref(s: &str) -> Result<scootlens_abi::ElementRef, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))
}

fn parse_url(s: &str) -> Result<url::Url, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("invalid url: {e}")))
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn method_not_found(id: RpcId, m: &str) -> RpcResponse {
    RpcResponse {
        jsonrpc: scootlens_abi::V2,
        id,
        outcome: RpcOutcome::Failure {
            error: RpcError {
                code: -32601,
                message: format!("method not found: {m}"),
                data: json!({ "code": "METHOD_NOT_FOUND" }),
            },
        },
    }
}
