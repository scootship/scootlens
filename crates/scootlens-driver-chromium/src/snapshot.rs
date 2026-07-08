//! CDP Accessibility 树 → HAL `A11yNode` 的转换与剪枝。
//!
//! 规则（docs/05-engine-hal.md 语义快照）：
//! - 交互角色（link/button/textbox/…）分配 `ElementRef`，映射到 `backendDOMNodeId`
//! - 结构角色（document/heading/list/…）保留但无 ref
//! - `ignored`、无名容器（generic 等）透传：子节点提升到父
//! - 超出 `max_nodes` 即截断（`truncated = true`）

use std::collections::HashMap;

use scootlens_abi::ElementRef;
use scootlens_hal::A11yNode;
use serde_json::Value;

/// 交互角色：分配 ref。
const INTERACTIVE: &[&str] = &[
    "link", "button", "textbox", "searchbox", "checkbox", "radio", "combobox", "menuitem",
    "tab", "switch", "slider", "spinbutton",
];

/// 结构角色：保留但无 ref。
const STRUCTURAL: &[&str] = &[
    "document", "heading", "paragraph", "list", "listitem", "image", "navigation", "main",
    "form", "table", "row", "cell", "text", "label", "banner", "contentinfo", "article",
    "region", "alert", "dialog", "status",
];

/// 剪枝转换结果。
pub(crate) struct Converted {
    pub root: A11yNode,
    pub truncated: bool,
    /// ref index → CDP backendDOMNodeId。
    pub backend_ids: HashMap<u64, i64>,
}

/// `Accessibility.getFullAXTree` 的 `nodes` 数组 → 剪枝树。
pub(crate) fn convert(nodes: &[Value], generation: u64, max_nodes: usize) -> Converted {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|n| Some((n.get("nodeId")?.as_str()?, n)))
        .collect();

    let mut cx = Cx {
        by_id,
        generation,
        max_nodes,
        count: 0,
        next_index: 1,
        truncated: false,
        backend_ids: HashMap::new(),
    };

    let root = match nodes.first().and_then(|r| cx.build(r)) {
        Some(mut v) if v.len() == 1 => v.remove(0),
        other => A11yNode {
            // 极端情形（root 被剪掉）：合成空 document 兜底
            elem_ref: None,
            role: "document".to_owned(),
            name: String::new(),
            value: None,
            children: other.unwrap_or_default(),
        },
    };

    Converted {
        root,
        truncated: cx.truncated,
        backend_ids: cx.backend_ids,
    }
}

struct Cx<'a> {
    by_id: HashMap<&'a str, &'a Value>,
    generation: u64,
    max_nodes: usize,
    count: usize,
    next_index: u64,
    truncated: bool,
    backend_ids: HashMap<u64, i64>,
}

impl Cx<'_> {
    /// 返回：Some(vec) —— 本节点产出的节点列表（保留节点 1 个；透传节点 = 子节点列表）。
    fn build(&mut self, raw: &Value) -> Option<Vec<A11yNode>> {
        if self.count >= self.max_nodes {
            self.truncated = true;
            return Some(Vec::new());
        }
        let ignored = raw.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let role = normalize_role(
            raw.pointer("/role/value")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let name = raw
            .pointer("/name/value")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim() // accname 空白归一化（label 布局空格不属于名字）
            .to_owned();

        let interactive = INTERACTIVE.contains(&role.as_str());
        let structural = STRUCTURAL.contains(&role.as_str());
        let keep = !ignored && (interactive || (structural && !name.is_empty()) || role == "document");

        let mut children = Vec::new();
        if let Some(ids) = raw.get("childIds").and_then(Value::as_array) {
            for id in ids {
                let Some(id) = id.as_str() else { continue };
                let Some(child_raw) = self.by_id.get(id).copied() else {
                    continue;
                };
                if let Some(mut out) = self.build(child_raw) {
                    children.append(&mut out);
                }
            }
        }

        if !keep {
            return Some(children); // 透传：子节点提升
        }

        self.count += 1;
        let elem_ref = if interactive {
            let index = self.next_index;
            self.next_index += 1;
            if let Some(backend) = raw.get("backendDOMNodeId").and_then(Value::as_i64) {
                self.backend_ids.insert(index, backend);
            }
            Some(ElementRef::new(self.generation, index))
        } else {
            None
        };
        let value = raw
            .pointer("/value/value")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                other => other.as_i64().map(|n| n.to_string()),
            })
            .filter(|s| !s.is_empty());

        Some(vec![A11yNode {
            elem_ref,
            role,
            name,
            value,
            children,
        }])
    }
}

/// CDP 角色名 → HAL 规范角色名。
fn normalize_role(cdp: &str) -> String {
    match cdp {
        "RootWebArea" => "document".to_owned(),
        "StaticText" => "text".to_owned(),
        "LabelText" => "label".to_owned(),
        "GenericContainer" | "generic" | "none" | "InlineTextBox" | "LineBreak" => {
            "generic".to_owned()
        }
        other => other.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ax_node(id: &str, role: &str, name: &str, children: &[&str]) -> Value {
        json!({
            "nodeId": id,
            "ignored": false,
            "role": { "type": "role", "value": role },
            "name": { "type": "computedString", "value": name },
            "childIds": children,
            "backendDOMNodeId": id.parse::<i64>().unwrap_or(0) + 100,
        })
    }

    #[test]
    fn converts_login_like_tree() {
        let nodes = vec![
            ax_node("1", "RootWebArea", "Login", &["2", "3", "4"]),
            ax_node("2", "textbox", "Username", &[]),
            ax_node("3", "textbox", "Password", &[]),
            ax_node("4", "button", "Sign in", &[]),
        ];
        let c = convert(&nodes, 7, 100);
        assert_eq!(c.root.role, "document");
        assert_eq!(c.root.name, "Login");
        assert_eq!(c.root.children.len(), 3);
        let user = &c.root.children[0];
        assert_eq!(user.role, "textbox");
        let r = user.elem_ref.clone().expect("ref");
        assert_eq!(r.generation(), 7);
        assert_eq!(c.backend_ids.get(&r.index()), Some(&102));
    }

    #[test]
    fn generic_containers_are_flattened() {
        let nodes = vec![
            ax_node("1", "RootWebArea", "Home", &["2"]),
            ax_node("2", "generic", "", &["3"]),
            ax_node("3", "link", "Go to Login", &[]),
        ];
        let c = convert(&nodes, 1, 100);
        assert_eq!(c.root.children.len(), 1);
        assert_eq!(c.root.children[0].role, "link");
    }

    #[test]
    fn ignored_nodes_lift_children() {
        let mut ignored = ax_node("2", "paragraph", "wrapper", &["3"]);
        ignored["ignored"] = json!(true);
        let nodes = vec![
            ax_node("1", "RootWebArea", "Home", &["2"]),
            ignored,
            ax_node("3", "heading", "Fixture Home", &[]),
        ];
        let c = convert(&nodes, 1, 100);
        assert_eq!(c.root.children[0].role, "heading");
    }

    #[test]
    fn truncation_flags_and_stops() {
        let nodes = vec![
            ax_node("1", "RootWebArea", "Home", &["2", "3", "4"]),
            ax_node("2", "link", "A", &[]),
            ax_node("3", "link", "B", &[]),
            ax_node("4", "link", "C", &[]),
        ];
        let c = convert(&nodes, 1, 2);
        assert!(c.truncated);
    }

    #[test]
    fn unnamed_structural_nodes_are_flattened() {
        let nodes = vec![
            ax_node("1", "RootWebArea", "Home", &["2"]),
            ax_node("2", "paragraph", "", &["3"]),
            ax_node("3", "link", "X", &[]),
        ];
        let c = convert(&nodes, 1, 100);
        assert_eq!(c.root.children[0].role, "link");
    }
}
