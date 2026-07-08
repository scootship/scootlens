//! Workflow Daemon（docs/09-roadmap.md P3）：cron / 事件 / 手动触发，
//! 每步经 Dispatcher 分发——journal 与鉴权免费获得；运行主体为受限的
//! `wf:<name>`，只持有 spec 声明的作用域（最小权限，创建时校验防提权）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use scootlens_abi::{
    AbiError, ErrorCode, RpcId, RpcOutcome, RpcRequest, Scope, WfSpec, WfStep, WfTrigger,
};
use serde_json::{Value, json};

use crate::bus::{BusPayload, WfRunStatus};
use crate::dispatch::Dispatcher;
use crate::security::Caller;

/// 注入时钟（unix 秒）；测试可替换为受控时钟。
pub type WfClock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub(crate) fn system_clock() -> WfClock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

/// cron 触发的评估节拍。
const CRON_TICK: Duration = Duration::from_secs(30);

struct WfEntry {
    spec: WfSpec,
    /// 运行主体：`wf:<name>` + spec 声明的作用域 + 创建者的审批约束。
    runner: Caller,
    trigger_task: Option<tokio::task::JoinHandle<()>>,
    /// 单实例运行防重入。
    running: Arc<AtomicBool>,
}

/// Workflow Daemon。由 [`Dispatcher`] 持有。
pub(crate) struct WfDaemon {
    wfs: Mutex<HashMap<String, WfEntry>>,
    clock: WfClock,
    run_counter: AtomicU64,
}

impl WfDaemon {
    pub(crate) fn new(clock: WfClock) -> Self {
        Self {
            wfs: Mutex::new(HashMap::new()),
            clock,
            run_counter: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, WfEntry>> {
        self.wfs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 注册工作流。拒绝：重名、非法名、空步骤、坏 cron、**超出创建者权限的作用域**。
    pub(crate) fn create(
        &self,
        dispatcher: &Dispatcher,
        creator: &Caller,
        spec: WfSpec,
    ) -> Result<(), AbiError> {
        validate_wf_name(&spec.name)?;
        if spec.steps.is_empty() {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                "workflow needs at least one step",
            ));
        }
        // 最小权限闭包：spec.scopes ⊆ 创建者有效作用域，工作流不可能比创建者更强
        let mut scopes = Vec::new();
        let effective = dispatcher.kernel().security().effective_scopes(creator);
        for s in &spec.scopes {
            let scope: Scope = s.parse().map_err(|e| {
                AbiError::new(ErrorCode::InvalidArg, format!("bad scope {s:?}: {e}"))
            })?;
            if !effective.iter().any(|g| g.covers(&scope)) {
                return Err(AbiError::new(
                    ErrorCode::CapDenied,
                    format!("workflow scope {s} exceeds creator's grants"),
                ));
            }
            scopes.push(scope);
        }
        if let WfTrigger::Cron { expr } = &spec.trigger {
            CronExpr::parse(expr)?; // fail fast
        }

        let mut wfs = self.lock();
        if wfs.contains_key(&spec.name) {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("workflow {:?} already exists", spec.name),
            ));
        }
        let runner = Caller {
            subject: format!("wf:{}", spec.name),
            scopes,
            constraints: creator.constraints.clone(),
        };
        let running = Arc::new(AtomicBool::new(false));
        let trigger_task = self.spawn_trigger(dispatcher, &spec, &runner, &running);
        wfs.insert(
            spec.name.clone(),
            WfEntry {
                spec,
                runner,
                trigger_task,
                running,
            },
        );
        Ok(())
    }

    fn spawn_trigger(
        &self,
        dispatcher: &Dispatcher,
        spec: &WfSpec,
        runner: &Caller,
        running: &Arc<AtomicBool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        match &spec.trigger {
            WfTrigger::Manual => None,
            WfTrigger::Cron { expr } => {
                let Ok(cron) = CronExpr::parse(expr) else {
                    return None; // create 已校验，防御性兜底
                };
                let d = dispatcher.clone();
                let clock = Arc::clone(&self.clock);
                let spec = spec.clone();
                let runner = runner.clone();
                let running = Arc::clone(running);
                Some(tokio::spawn(async move {
                    let mut last_fired_minute: Option<u64> = None;
                    loop {
                        tokio::time::sleep(CRON_TICK).await;
                        let now = clock();
                        let minute = now / 60;
                        if last_fired_minute == Some(minute) || !cron.matches(now) {
                            continue;
                        }
                        last_fired_minute = Some(minute);
                        run_guarded(&d, &spec, &runner, &running).await;
                    }
                }))
            }
            WfTrigger::Event { topic } => {
                let mut rx = dispatcher.kernel().subscribe();
                let d = dispatcher.clone();
                let topic = topic.clone();
                let spec = spec.clone();
                let runner = runner.clone();
                let running = Arc::clone(running);
                Some(tokio::spawn(async move {
                    while let Ok(event) = rx.recv().await {
                        if event.payload.topic() == topic {
                            run_guarded(&d, &spec, &runner, &running).await;
                        }
                    }
                }))
            }
        }
    }

    /// 手动触发：同步跑完（调用内），返回完成步数。
    pub(crate) async fn run(&self, dispatcher: &Dispatcher, name: &str) -> Result<Value, AbiError> {
        let (spec, runner, running) = {
            let wfs = self.lock();
            let e = wfs.get(name).ok_or_else(|| wf_not_found(name))?;
            (e.spec.clone(), e.runner.clone(), Arc::clone(&e.running))
        };
        if running.swap(true, Ordering::SeqCst) {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("workflow {name:?} is already running"),
            ));
        }
        let run_id = self.run_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let outcome = execute(dispatcher, &spec, &runner).await;
        running.store(false, Ordering::SeqCst);
        match outcome {
            Ok(steps) => Ok(json!({ "ok": true, "run_id": run_id, "steps": steps })),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn list(&self) -> Value {
        let wfs = self.lock();
        let mut items: Vec<Value> = wfs
            .values()
            .map(|e| {
                json!({
                    "name": e.spec.name,
                    "trigger": e.spec.trigger,
                    "steps": e.spec.steps.len(),
                    "scopes": e.spec.scopes,
                    "running": e.running.load(Ordering::SeqCst),
                })
            })
            .collect();
        items.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_owned());
        json!({ "workflows": items })
    }

    /// 注销工作流并停掉触发器。运行中的手动 run 自然结束（不强杀）。
    pub(crate) fn cancel(&self, name: &str) -> Result<(), AbiError> {
        let entry = self.lock().remove(name).ok_or_else(|| wf_not_found(name))?;
        if let Some(t) = entry.trigger_task {
            t.abort();
        }
        Ok(())
    }
}

impl Drop for WfDaemon {
    fn drop(&mut self) {
        for e in self.lock().values() {
            if let Some(t) = &e.trigger_task {
                t.abort();
            }
        }
    }
}

fn wf_not_found(name: &str) -> AbiError {
    AbiError::new(ErrorCode::InvalidArg, format!("no such workflow: {name}"))
}

fn validate_wf_name(name: &str) -> Result<(), AbiError> {
    let mut chars = name.chars();
    let head_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if head_ok && rest_ok && name.len() <= 64 {
        Ok(())
    } else {
        Err(AbiError::new(
            ErrorCode::InvalidArg,
            format!("invalid workflow name {name:?} (want [A-Za-z0-9][A-Za-z0-9._-]*, <=64 chars)"),
        ))
    }
}

/// 触发器驱动的运行：占用运行位失败（已在跑）则跳过本次触发。
async fn run_guarded(d: &Dispatcher, spec: &WfSpec, runner: &Caller, running: &AtomicBool) {
    if running.swap(true, Ordering::SeqCst) {
        tracing::debug!(wf = %spec.name, "trigger skipped: already running");
        return;
    }
    if let Err(e) = execute(d, spec, runner).await {
        tracing::warn!(wf = %spec.name, %e, "workflow run failed");
    }
    running.store(false, Ordering::SeqCst);
}

/// 依序执行步骤；每步经 Dispatcher（journal/鉴权全覆盖），失败按退避重试。
async fn execute(d: &Dispatcher, spec: &WfSpec, runner: &Caller) -> Result<u32, AbiError> {
    let kernel = d.kernel();
    kernel.emit_wf(&spec.name, WfRunStatus::Started, None, None);
    for (idx, step) in spec.steps.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let step_no = idx as u32;
        match run_step(d, runner, step, &spec.name, step_no).await {
            Ok(()) => kernel.emit_wf(&spec.name, WfRunStatus::StepOk, Some(step_no), None),
            Err(e) => {
                kernel.emit_wf(
                    &spec.name,
                    WfRunStatus::Failed,
                    Some(step_no),
                    Some(e.to_string()),
                );
                return Err(e);
            }
        }
    }
    kernel.emit_wf(&spec.name, WfRunStatus::Succeeded, None, None);
    #[allow(clippy::cast_possible_truncation)]
    Ok(spec.steps.len() as u32)
}

/// 单步执行 + 指数退避重试（backoff_ms、2x、4x…）。
async fn run_step(
    d: &Dispatcher,
    runner: &Caller,
    step: &WfStep,
    wf_name: &str,
    step_no: u32,
) -> Result<(), AbiError> {
    let mut attempt: u32 = 0;
    loop {
        let req = RpcRequest::new(
            RpcId::Str(format!("wf-{wf_name}-{step_no}-{attempt}")),
            step.method.clone(),
            step.params.clone(),
        );
        let resp = Box::pin(d.dispatch(runner, req)).await;
        match resp.outcome {
            RpcOutcome::Success { .. } => return Ok(()),
            RpcOutcome::Failure { error } => {
                if attempt >= step.retry.max_attempts {
                    return Err(AbiError::new(
                        ErrorCode::Internal,
                        format!(
                            "step {step_no} ({}) failed after {attempt} retries: {}",
                            step.method, error.message
                        ),
                    ));
                }
                let backoff = step.retry.backoff_ms.saturating_mul(1 << attempt.min(16));
                d.kernel().emit_wf(
                    wf_name,
                    WfRunStatus::StepRetry,
                    Some(step_no),
                    Some(error.message.clone()),
                );
                tokio::time::sleep(Duration::from_millis(backoff)).await;
                attempt += 1;
            }
        }
    }
}

impl crate::Kernel {
    /// 发 `wf.run` 事件（daemon 专用便捷入口）。
    pub(crate) fn emit_wf(
        &self,
        wf: &str,
        status: WfRunStatus,
        step: Option<u32>,
        detail: Option<String>,
    ) {
        self.emit(
            None,
            BusPayload::WfRun {
                wf: wf.to_owned(),
                status,
                step,
                detail,
            },
        );
    }
}

// ---------- cron（5 段：分 时 日 月 星期；UTC） ----------

/// 已解析的 cron 表达式。支持 `*`、`*/n`、`a`、`a-b`、`a-b/n`、逗号列表。
///
/// 语义遵循传统 cron：日与星期都受限时任一匹配即可；星期 0/7 均为周日。
struct CronExpr {
    minute: CronField,
    hour: CronField,
    dom: CronField,
    month: CronField,
    dow: CronField,
}

struct CronField {
    /// None = `*`（无约束）。
    allowed: Option<Vec<u32>>,
}

impl CronField {
    fn contains(&self, v: u32) -> bool {
        self.allowed.as_ref().is_none_or(|a| a.contains(&v))
    }

    fn is_wildcard(&self) -> bool {
        self.allowed.is_none()
    }
}

impl CronExpr {
    fn parse(expr: &str) -> Result<Self, AbiError> {
        let bad = |why: String| AbiError::new(ErrorCode::InvalidArg, why);
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let [minute, hour, dom, month, dow] = fields.as_slice() else {
            return Err(bad(format!(
                "cron expr must have 5 fields, got {}: {expr:?}",
                fields.len()
            )));
        };
        Ok(Self {
            minute: parse_field(minute, 0, 59)?,
            hour: parse_field(hour, 0, 23)?,
            dom: parse_field(dom, 1, 31)?,
            month: parse_field(month, 1, 12)?,
            dow: parse_dow(dow)?,
        })
    }

    /// unix 秒（UTC）是否命中。
    fn matches(&self, unix_secs: u64) -> bool {
        let t = CivilTime::from_unix(unix_secs);
        let time_ok = self.minute.contains(t.minute)
            && self.hour.contains(t.hour)
            && self.month.contains(t.month);
        if !time_ok {
            return false;
        }
        // 传统 cron 语义：dom 与 dow 均受限 → 或；否则 → 与
        let dom_hit = self.dom.contains(t.dom);
        let dow_hit = self.dow.contains(t.dow);
        match (self.dom.is_wildcard(), self.dow.is_wildcard()) {
            (false, false) => dom_hit || dow_hit,
            _ => dom_hit && dow_hit,
        }
    }
}

fn parse_dow(text: &str) -> Result<CronField, AbiError> {
    // 星期字段允许 7 = 周日
    let f = parse_field(text, 0, 7)?;
    Ok(CronField {
        allowed: f
            .allowed
            .map(|v| v.into_iter().map(|d| if d == 7 { 0 } else { d }).collect()),
    })
}

fn parse_field(text: &str, min: u32, max: u32) -> Result<CronField, AbiError> {
    let bad = |why: String| AbiError::new(ErrorCode::InvalidArg, why);
    if text == "*" {
        return Ok(CronField { allowed: None });
    }
    let mut allowed = Vec::new();
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| bad(format!("bad cron step {s:?} in {text:?}")))?;
                if step == 0 {
                    return Err(bad(format!("cron step 0 in {text:?}")));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            let lo = a
                .parse()
                .map_err(|_| bad(format!("bad cron range {range:?} in {text:?}")))?;
            let hi = b
                .parse()
                .map_err(|_| bad(format!("bad cron range {range:?} in {text:?}")))?;
            (lo, hi)
        } else {
            let v = range
                .parse()
                .map_err(|_| bad(format!("bad cron value {range:?} in {text:?}")))?;
            (v, v)
        };
        if lo < min || hi > max || lo > hi {
            return Err(bad(format!(
                "cron value out of range {min}..={max}: {text:?}"
            )));
        }
        allowed.extend((lo..=hi).step_by(step as usize));
    }
    allowed.sort_unstable();
    allowed.dedup();
    Ok(CronField {
        allowed: Some(allowed),
    })
}

/// UTC 民用时间分解（Howard Hinnant civil-from-days）。
struct CivilTime {
    minute: u32,
    hour: u32,
    /// 月内日（1 起）。
    dom: u32,
    month: u32,
    /// 星期（0 = 周日）。
    dow: u32,
}

impl CivilTime {
    fn from_unix(secs: u64) -> Self {
        let days = (secs / 86_400) as i64;
        let rem = secs % 86_400;
        let hour = (rem / 3600) as u32;
        let minute = (rem % 3600 / 60) as u32;
        // 1970-01-01 是周四（dow=4）
        let dow = ((days + 4).rem_euclid(7)) as u32;

        // civil_from_days
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let dom = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        let _year = yoe + era * 400 + i64::from(month <= 2);
        Self {
            minute,
            hour,
            dom,
            month,
            dow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_parses_and_matches_known_instants() {
        // 2024-01-15 是周一；10:30 UTC = unix 1705314600
        let ts = 1_705_314_600;
        for (expr, want) in [
            ("* * * * *", true),
            ("30 10 * * *", true),
            ("30 10 15 1 *", true),
            ("30 10 * * 1", true),  // 周一
            ("30 10 * * 0", false), // 周日不匹配
            ("*/15 * * * *", true), // 30 可被 15 整除
            ("*/7 * * * *", false), // 30 不在 0,7,14,21,28,35…
            ("0 0 * * *", false),
            ("30 10-12 * * *", true),
            ("30 8,10 * * *", true),
            // dom 与 dow 均受限：任一匹配即可（15 号命中，周日不命中）
            ("30 10 15 * 0", true),
            ("30 10 16 * 0", false),
        ] {
            let cron = CronExpr::parse(expr).expect(expr);
            assert_eq!(cron.matches(ts), want, "expr {expr:?}");
        }
    }

    #[test]
    fn cron_rejects_malformed_expressions() {
        for expr in [
            "",
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "*/0 * * * *",
            "a * * * *",
        ] {
            assert!(CronExpr::parse(expr).is_err(), "expr {expr:?} must fail");
        }
    }

    #[test]
    fn dow_seven_means_sunday() {
        // 2024-01-14 10:30 UTC 是周日
        let sunday = 1_705_228_200;
        let cron = CronExpr::parse("30 10 * * 7").expect("parse");
        assert!(cron.matches(sunday));
    }
}
