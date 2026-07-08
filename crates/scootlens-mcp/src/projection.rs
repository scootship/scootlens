//! ABI 投影：工具清单从 [`scootlens_abi::method::ALL`] 生成。
//!
//! 命名规则（docs/03-abi-spec.md §MCP 投影）：`scootlens_<domain>_<verb>`，
//! 即方法名中的 `.` → `_`。连接级方法（`evt.subscribe/unsubscribe`）不投影：
//! 订阅是 gateway 会话语义，MCP 客户端用 `scootlens_evt_wait` 做条件等待。
//!
//! 本层不含任何权限判断——作用域/审批全部由内核 Security Manager 强制。

use scootlens_abi::method;

/// 不投影的连接级方法。
pub const EXCLUDED_METHODS: &[&str] = &[method::EVT_SUBSCRIBE, method::EVT_UNSUBSCRIBE];

/// 一个投影工具。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDef {
    /// MCP 工具名（`scootlens_proc_spawn`）。
    pub name: String,
    /// 对应 ABI 方法（`proc.spawn`）。
    pub method: &'static str,
    /// 工具描述。
    pub description: String,
}

/// ABI 方法名 → MCP 工具名。
pub fn tool_name(method: &str) -> String {
    format!("scootlens_{}", method.replace('.', "_"))
}

/// 全量工具表（顺序与方法表一致）。
pub fn tool_defs() -> Vec<ToolDef> {
    method::ALL
        .iter()
        .filter(|m| !EXCLUDED_METHODS.contains(m))
        .map(|m| ToolDef {
            name: tool_name(m),
            method: m,
            description: describe(m),
        })
        .collect()
}

/// MCP 工具名 → ABI 方法名（未知工具返回 None）。
pub fn method_for_tool(tool: &str) -> Option<&'static str> {
    method::ALL
        .iter()
        .filter(|m| !EXCLUDED_METHODS.contains(m))
        .find(|m| tool_name(m) == tool)
        .copied()
}

/// 工具描述（提示词，不承载语义；权威定义见 docs/03-abi-spec.md）。
fn describe(m: &str) -> String {
    let text = match m {
        method::PROC_SPAWN => {
            "Spawn a web session process. Params: profile?, quotas?. Returns pid."
        }
        method::PROC_LIST => "List processes. Returns procs[].",
        method::PROC_INFO => "Process info. Params: pid.",
        method::PROC_KILL => "Terminate a process. Params: pid.",
        method::PROC_SUSPEND => "Suspend a running process. Params: pid.",
        method::PROC_RESUME => "Resume a suspended process. Params: pid.",
        method::PROC_SNAPSHOT => "Snapshot process state. Params: pid. Returns snap_id.",
        method::PROC_RESTORE => "Restore a process from a snapshot. Params: snap_id, engine?.",
        method::NAV_GOTO => "Navigate to a URL. Params: pid, url.",
        method::NAV_BACK => "History back. Params: pid.",
        method::NAV_FORWARD => "History forward. Params: pid.",
        method::NAV_RELOAD => "Reload current page. Params: pid.",
        method::VIEW_SNAPSHOT => {
            "Semantic accessibility snapshot with element refs. Params: pid, max_nodes?. \
             Returns generation + compact text; refs expire on next snapshot."
        }
        method::VIEW_SCREENSHOT => "PNG screenshot. Params: pid. Returns data_base64.",
        method::ACT_CLICK => "Click an element. Params: pid, ref.",
        method::ACT_TYPE => "Type into an element. Params: pid, ref, text | vault_ref.",
        method::ACT_PRESS => "Press keys. Params: pid, keys (e.g. 'Enter').",
        method::ACT_SCROLL => "Scroll page or element. Params: pid, ref?, dx, dy.",
        method::ACT_SELECT => {
            "Select dropdown option(s) by visible text. Params: pid, ref, values."
        }
        method::ACT_UPLOAD => "Upload a sandboxed file. Params: pid, ref, path.",
        method::ACT_TAKEOVER_START => {
            "Begin human takeover of a process: other subjects' inputs are held. Params: pid."
        }
        method::ACT_TAKEOVER_END => "End human takeover and release held inputs. Params: pid.",
        method::DOM_EXTRACT => "Extract nodes by role/name. Params: pid, role?, name?, max?.",
        method::JS_EXEC => {
            "Execute JavaScript (sensitive; may require approval). Params: pid, script, args?."
        }
        method::EVT_WAIT => "Wait for a condition. Params: pid, cond, timeout_ms.",
        method::STATE_READ => "Read state VFS. Params: namespace, pid?, key?.",
        method::STATE_WRITE => {
            "Write state VFS (vault is write-only). Params: namespace, pid?, key, value."
        }
        method::STATE_LIST => "List state VFS entries. Params: namespace, pid?.",
        method::STATE_EXPORT => "Export full session state bundle. Params: pid.",
        method::STATE_IMPORT => "Import a state bundle into a profile. Params: profile, state.",
        method::NET_RULES_SET => "Set network rules. Params: pid?, default?, rules?.",
        method::NET_RULES_GET => "Get network rules. Params: pid?.",
        method::NET_LOG => "Network request log. Params: pid?, limit?.",
        method::CAP_REQUEST => {
            "Request a capability scope (goes to approval inbox). Params: scope, reason?."
        }
        method::CAP_LIST => "List own subject and effective scopes.",
        method::CAP_GRANT => "Grant a scope to a subject (admin). Params: subject, scope.",
        method::CAP_REVOKE => "Revoke a scope from a subject (admin). Params: subject, scope.",
        method::CAP_APPROVE => {
            "Decide a pending approval (admin). Params: approval_id, decision, remember?."
        }
        method::CAP_PENDING => "List pending approvals (admin).",
        method::WF_CREATE => "Create a workflow. Params: spec.",
        method::WF_LIST => "List workflows.",
        method::WF_RUN => "Run a workflow now. Params: name.",
        method::WF_CANCEL => "Cancel/disable a workflow. Params: name.",
        method::OBS_JOURNAL => "Audit journal tail. Params: pid?, limit?.",
        method::OBS_TRACE => "Per-process syscall trace. Params: pid.",
        method::OBS_REPLAY_EXPORT => {
            "Export a replay bundle (journal hash-chain segment + frames). Params: pid, journal_limit?."
        }
        method::SYS_INFO => "Kernel/engine info and quota headroom.",
        other => return format!("ScootLens ABI call {other} (see docs/03-abi-spec.md)."),
    };
    text.to_owned()
}

/// 工具入参 schema：宽松对象（权威校验在内核，serde 强校验 → `E_INVALID_ARG`）。
pub fn input_schema(m: &str) -> serde_json::Map<String, serde_json::Value> {
    let schema = serde_json::json!({
        "type": "object",
        "description": format!("JSON-RPC params for `{m}` (authoritative spec: docs/03-abi-spec.md)"),
        "additionalProperties": true,
    });
    match schema {
        serde_json::Value::Object(o) => o,
        _ => unreachable!("schema literal is an object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_covers_method_table_minus_connection_scoped() {
        let defs = tool_defs();
        assert_eq!(defs.len(), method::ALL.len() - EXCLUDED_METHODS.len());
        assert!(defs.iter().all(|d| d.name.starts_with("scootlens_")));
        assert!(!defs.iter().any(|d| d.name.contains('.')));
        assert!(
            !defs
                .iter()
                .any(|d| d.method == method::EVT_SUBSCRIBE || d.method == method::EVT_UNSUBSCRIBE),
            "connection-scoped methods must not be projected"
        );
    }

    #[test]
    fn tool_names_map_back_bijectively() {
        for def in tool_defs() {
            assert_eq!(
                method_for_tool(&def.name),
                Some(def.method),
                "{} must round-trip",
                def.name
            );
        }
        assert_eq!(method_for_tool("scootlens_evt_subscribe"), None);
        assert_eq!(method_for_tool("bogus"), None);
    }

    #[test]
    fn multi_segment_methods_flatten() {
        assert_eq!(tool_name("net.rules.set"), "scootlens_net_rules_set");
        assert_eq!(
            method_for_tool("scootlens_obs_replay_export"),
            Some(method::OBS_REPLAY_EXPORT)
        );
        assert_eq!(
            method_for_tool("scootlens_act_takeover_start"),
            Some(method::ACT_TAKEOVER_START)
        );
    }

    #[test]
    fn every_tool_has_description_and_object_schema() {
        for def in tool_defs() {
            assert!(!def.description.is_empty());
            let schema = input_schema(def.method);
            assert_eq!(schema["type"], "object");
        }
    }
}
