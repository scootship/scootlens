//! 内核网络栈：分层规则（global + per-proc）、逐请求判定、`net.log` 环形缓冲。
//!
//! 规则引擎在 scootlens-net；强制点在引擎侧——驱动通过
//! [`ProcPolicy`]（[`scootlens_hal::RequestPolicy`] 实现）逐请求询问。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use scootlens_abi::{NetDecision, NetRequestSummary, NetRuleSet, Pid};
use scootlens_hal::RequestPolicy;
use serde_json::{Value, json};

const LOG_CAP: usize = 1024;

/// 网络栈。
#[derive(Default)]
pub struct NetStack {
    global: Mutex<Option<NetRuleSet>>,
    per_proc: Mutex<HashMap<Pid, NetRuleSet>>,
    log: Mutex<VecDeque<Value>>,
}

impl NetStack {
    /// 设置规则：`pid = None` → 全局层。
    pub fn set_rules(&self, pid: Option<&Pid>, rules: NetRuleSet) {
        match pid {
            Some(p) => {
                self.lock(&self.per_proc).insert(p.clone(), rules);
            }
            None => {
                *self.lock(&self.global) = Some(rules);
            }
        }
    }

    pub fn get_rules(&self, pid: Option<&Pid>) -> Option<NetRuleSet> {
        match pid {
            Some(p) => self.lock(&self.per_proc).get(p).cloned(),
            None => self.lock(&self.global).clone(),
        }
    }

    pub fn drop_proc(&self, pid: &Pid) {
        self.lock(&self.per_proc).remove(pid);
    }

    /// 逐请求判定 + 记录 `net.log`。
    pub fn decide(&self, pid: &Pid, req: &NetRequestSummary) -> NetDecision {
        let decision = {
            let per_proc = self.lock(&self.per_proc);
            let global = self.lock(&self.global);
            scootlens_net::evaluate_layered(per_proc.get(pid), global.as_ref(), req)
        };
        let mut log = self.lock(&self.log);
        if log.len() >= LOG_CAP {
            log.pop_front();
        }
        log.push_back(json!({
            "ts_ms": crate::security::unix_now_ms(),
            "pid": pid.to_string(),
            "url": req.url,
            "method": req.method,
            "resource_type": req.resource_type,
            "allowed": decision.allowed(),
        }));
        decision
    }

    /// 请求日志（新→旧，最多 `limit` 条，可选 pid 过滤）。
    pub fn log(&self, pid: Option<&Pid>, limit: usize) -> Vec<Value> {
        let want = pid.map(std::string::ToString::to_string);
        self.lock(&self.log)
            .iter()
            .rev()
            .filter(|e| match &want {
                Some(p) => e.get("pid").and_then(Value::as_str) == Some(p),
                None => true,
            })
            .take(limit)
            .cloned()
            .collect()
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 单进程视角的请求策略（装进驱动）。
pub struct ProcPolicy {
    stack: Arc<NetStack>,
    pid: Pid,
}

impl ProcPolicy {
    pub fn new(stack: Arc<NetStack>, pid: Pid) -> Self {
        Self { stack, pid }
    }
}

impl RequestPolicy for ProcPolicy {
    fn decide(&self, req: &NetRequestSummary) -> NetDecision {
        self.stack.decide(&self.pid, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scootlens_abi::{NetAction, NetDefault, NetRule};

    fn pid(s: &str) -> Pid {
        s.parse().expect("pid")
    }

    fn req(host: &str) -> NetRequestSummary {
        NetRequestSummary {
            url: format!("http://{host}/x"),
            method: "GET".into(),
            resource_type: "document".into(),
        }
    }

    fn allow(host: &str) -> NetRule {
        NetRule {
            action: NetAction::Allow,
            host: host.into(),
            methods: vec![],
            resource_types: vec![],
            set_headers: vec![],
        }
    }

    #[test]
    fn per_proc_allowlist_layers_over_global_deny() {
        let stack = NetStack::default();
        let p = pid("p-alpha");
        let other = pid("p-beta");
        stack.set_rules(
            None,
            NetRuleSet {
                default: NetDefault::Deny,
                rules: vec![],
            },
        );
        stack.set_rules(
            Some(&p),
            NetRuleSet {
                default: NetDefault::Deny,
                rules: vec![allow("api.test")],
            },
        );
        assert!(
            stack.decide(&p, &req("api.test")).allowed(),
            "proc allowlist hit"
        );
        assert!(
            !stack.decide(&p, &req("unlisted.test")).allowed(),
            "global deny still applies"
        );
        assert!(
            !stack.decide(&other, &req("api.test")).allowed(),
            "other proc sees global deny"
        );
    }

    #[test]
    fn decide_records_log_newest_first() {
        let stack = NetStack::default();
        let p = pid("p-log");
        stack.set_rules(
            None,
            NetRuleSet {
                default: NetDefault::Allow,
                rules: vec![],
            },
        );
        stack.decide(&p, &req("a.test"));
        stack.decide(&p, &req("b.test"));
        let entries = stack.log(Some(&p), 10);
        assert_eq!(entries.len(), 2);
        assert!(
            entries[0]["url"].as_str().expect("url").contains("b.test"),
            "newest first"
        );
        assert_eq!(entries[0]["allowed"], true);
    }

    #[test]
    fn drop_proc_clears_per_proc_rules() {
        let stack = NetStack::default();
        let p = pid("p-gone");
        stack.set_rules(
            Some(&p),
            NetRuleSet {
                default: NetDefault::Deny,
                rules: vec![],
            },
        );
        assert!(stack.get_rules(Some(&p)).is_some());
        stack.drop_proc(&p);
        assert!(stack.get_rules(Some(&p)).is_none());
    }
}
