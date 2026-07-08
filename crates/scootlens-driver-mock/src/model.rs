//! 可编程页面模型。

use std::collections::HashMap;

use scootlens_abi::ElementRef;
use scootlens_hal::A11yNode;
use url::Url;

/// 站点模型：url 路径 → 页面。
pub struct SiteModel {
    base: Url,
    pages: HashMap<String, PageModel>,
    not_found: PageModel,
}

impl SiteModel {
    /// 按 url 解析页面；未知路径返回内建 404 页（贴近真实引擎行为）。
    pub fn resolve(&self, url: &Url) -> &PageModel {
        self.pages.get(url.path()).unwrap_or(&self.not_found)
    }

    /// 初始空白页地址。
    pub fn blank_url(&self) -> Url {
        self.base.join("/__blank").expect("static path")
    }
}

/// 站点构建器。
pub struct SiteBuilder {
    base: Url,
    pages: HashMap<String, PageModel>,
}

impl SiteBuilder {
    pub fn new(base: Url) -> Self {
        Self {
            base,
            pages: HashMap::new(),
        }
    }

    pub fn page(mut self, path: &str, page: PageModel) -> Self {
        self.pages.insert(path.to_owned(), page);
        self
    }

    pub fn build(mut self) -> SiteModel {
        self.pages
            .entry("/__blank".to_owned())
            .or_insert_with(|| PageModel::document("about:blank"));
        SiteModel {
            base: self.base,
            pages: self.pages,
            not_found: PageModel::document("Not Found").child(NodeModel::heading("404 Not Found")),
        }
    }
}

/// 页面模型：标题 + 节点树。
pub struct PageModel {
    pub title: String,
    pub root: NodeModel,
}

impl PageModel {
    pub fn document(title: &str) -> Self {
        Self {
            title: title.to_owned(),
            root: NodeModel {
                role: "document".into(),
                name: title.to_owned(),
                on_click: None,
                interactive: false,
                children: vec![],
            },
        }
    }

    pub fn child(mut self, node: NodeModel) -> Self {
        self.root.children.push(node);
        self
    }

    /// 按索引路径取节点（空路径 = root）。
    pub fn node_at(&self, path: &[usize]) -> Option<&NodeModel> {
        let mut cur = &self.root;
        for &i in path {
            cur = cur.children.get(i)?;
        }
        Some(cur)
    }
}

/// 节点模型。
pub struct NodeModel {
    pub role: String,
    pub name: String,
    /// 点击后导航目标（相对路径），None 表示点击无导航。
    pub on_click: Option<String>,
    pub interactive: bool,
    pub children: Vec<NodeModel>,
}

impl NodeModel {
    pub fn heading(name: &str) -> Self {
        Self::plain("heading", name)
    }

    pub fn text(name: &str) -> Self {
        Self::plain("text", name)
    }

    pub fn link(name: &str, to: &str) -> Self {
        Self {
            role: "link".into(),
            name: name.to_owned(),
            on_click: Some(to.to_owned()),
            interactive: true,
            children: vec![],
        }
    }

    pub fn button(name: &str, to: Option<&str>) -> Self {
        Self {
            role: "button".into(),
            name: name.to_owned(),
            on_click: to.map(str::to_owned),
            interactive: true,
            children: vec![],
        }
    }

    pub fn textbox(name: &str) -> Self {
        Self {
            role: "textbox".into(),
            name: name.to_owned(),
            on_click: None,
            interactive: true,
            children: vec![],
        }
    }

    pub fn group(name: &str, children: Vec<NodeModel>) -> Self {
        Self {
            role: "group".into(),
            name: name.to_owned(),
            on_click: None,
            interactive: false,
            children,
        }
    }

    fn plain(role: &str, name: &str) -> Self {
        Self {
            role: role.into(),
            name: name.to_owned(),
            on_click: None,
            interactive: false,
            children: vec![],
        }
    }
}

/// 渲染语义快照：DFS 为交互节点分配 ref，应用输入值 overlay，按 max_nodes 截断。
pub fn render_snapshot(
    page: &PageModel,
    generation: u64,
    max_nodes: usize,
    page_url: &Url,
    values: &HashMap<(Url, Vec<usize>), String>,
) -> (A11yNode, HashMap<u64, Vec<usize>>, bool) {
    let mut ctx = RenderCtx {
        generation,
        max_nodes,
        page_url,
        values,
        next_index: 0,
        count: 0,
        truncated: false,
        ref_paths: HashMap::new(),
    };
    let root = ctx.render(&page.root, &mut Vec::new());
    (root, ctx.ref_paths, ctx.truncated)
}

struct RenderCtx<'a> {
    generation: u64,
    max_nodes: usize,
    page_url: &'a Url,
    values: &'a HashMap<(Url, Vec<usize>), String>,
    next_index: u64,
    count: usize,
    truncated: bool,
    ref_paths: HashMap<u64, Vec<usize>>,
}

impl RenderCtx<'_> {
    fn render(&mut self, node: &NodeModel, path: &mut Vec<usize>) -> A11yNode {
        self.count += 1;
        let elem_ref = if node.interactive {
            let idx = self.next_index;
            self.next_index += 1;
            self.ref_paths.insert(idx, path.clone());
            Some(ElementRef::new(self.generation, idx))
        } else {
            None
        };
        let value = self
            .values
            .get(&(self.page_url.clone(), path.clone()))
            .cloned();

        let mut children = Vec::new();
        for (i, c) in node.children.iter().enumerate() {
            if self.count >= self.max_nodes {
                self.truncated = true;
                break;
            }
            path.push(i);
            children.push(self.render(c, path));
            path.pop();
        }

        A11yNode {
            elem_ref,
            role: node.role.clone(),
            name: node.name.clone(),
            value,
            children,
        }
    }
}
