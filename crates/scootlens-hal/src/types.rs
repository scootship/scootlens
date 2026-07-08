//! HAL 数据类型。

use std::collections::BTreeMap;
use std::path::PathBuf;

use scootlens_abi::ElementRef;
use scootlens_abi::NetRequestSummary;
use serde::{Deserialize, Serialize};
use url::Url;

/// 进程 profile 规格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSpec {
    pub name: String,
    /// 下载落盘目录（State VFS `downloads/`）。None = 禁止下载。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_dir: Option<PathBuf>,
}

impl Default for ProfileSpec {
    fn default() -> Self {
        Self {
            name: "default".into(),
            download_dir: None,
        }
    }
}

/// 导航结果 / 当前页信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavResult {
    pub url: Url,
    pub title: String,
}

/// 历史移动方向（nav.back / nav.forward）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryDir {
    Back,
    Forward,
}

/// 语义快照选项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotOpts {
    /// 节点上限，超出即截断（docs/05-engine-hal.md）。
    pub max_nodes: usize,
}

impl Default for SnapshotOpts {
    fn default() -> Self {
        Self { max_nodes: 800 }
    }
}

/// 语义快照：带元素引用的精简可访问性树。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct A11ySnapshot {
    pub generation: u64,
    pub root: A11yNode,
    pub truncated: bool,
}

/// 快照节点。交互节点携带 `ElementRef`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct A11yNode {
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub elem_ref: Option<ElementRef>,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<A11yNode>,
}

impl A11ySnapshot {
    /// LLM 友好的紧凑文本格式（缩进树），是 `view.snapshot` 的主输出。
    pub fn to_compact_text(&self) -> String {
        let mut out = String::new();
        render_node(&self.root, 0, &mut out);
        if self.truncated {
            out.push_str("… (truncated)\n");
        }
        out
    }

    /// DFS 查找：按 role 与 name 找到第一个匹配节点。
    pub fn find(&self, role: &str, name: &str) -> Option<&A11yNode> {
        find_node(&self.root, role, name)
    }
}

fn render_node(n: &A11yNode, depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str("- ");
    out.push_str(&n.role);
    out.push_str(" \"");
    out.push_str(&n.name);
    out.push('"');
    if let Some(v) = &n.value {
        out.push_str(" = \"");
        out.push_str(v);
        out.push('"');
    }
    if let Some(r) = &n.elem_ref {
        out.push_str(" [");
        out.push_str(&r.to_string());
        out.push(']');
    }
    out.push('\n');
    for c in &n.children {
        render_node(c, depth + 1, out);
    }
}

fn find_node<'a>(n: &'a A11yNode, role: &str, name: &str) -> Option<&'a A11yNode> {
    if n.role == role && n.name == name {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_node(c, role, name))
}

/// 输入动作。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputAction {
    Click {
        target: ElementRef,
    },
    Type {
        target: ElementRef,
        text: String,
    },
    Press {
        keys: String,
    },
    Scroll {
        target: Option<ElementRef>,
        dx: f64,
        dy: f64,
    },
    /// 下拉选择：按选项可见文本匹配；多值仅对 multi-select 合法。
    Select {
        target: ElementRef,
        values: Vec<String>,
    },
    /// 文件上传：路径由内核解析并校验（沙箱 `downloads/` 内）后传入。
    Upload {
        target: ElementRef,
        paths: Vec<PathBuf>,
    },
}

/// 动作结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActResult {
    /// 本动作是否引发导航（客户端应重新 snapshot）。
    pub nav_occurred: bool,
}

/// 引擎状态包（cookie/storage 导出导入，P0 为不透明键值）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateBundle {
    pub entries: BTreeMap<String, serde_json::Value>,
}

/// 能力矩阵：驱动声明支持面，不支持的调用返回 `E_UNSUPPORTED`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EngineCaps {
    pub snapshot: bool,
    pub screenshot: bool,
    pub input: bool,
    pub eval: bool,
    pub net_rules: bool,
    pub state: bool,
    pub events: bool,
    pub metrics: bool,
}

/// 引擎指标（供 Scheduler 配额）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineMetrics {
    pub memory_bytes: u64,
}

/// 引擎事件（进入内核 Event Bus）。
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    Navigated {
        url: Url,
    },
    Crashed,
    ConsoleLog {
        text: String,
    },
    /// 一次网络请求经过策略判定（`net.log` 数据源）。
    NetRequest {
        summary: NetRequestSummary,
        allowed: bool,
    },
}
