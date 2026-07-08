//! 系统调用方法表 v0（docs/03-abi-spec.md）。
//!
//! 新增/改名必须走 ADR；契约测试以 golden 锁定全表。

macro_rules! methods {
    ($( $(#[$meta:meta])* $konst:ident = $name:literal; )+) => {
        $( $(#[$meta])* pub const $konst: &str = $name; )+

        /// 系统调用全表（顺序即文档顺序）。
        pub const ALL: &[&str] = &[ $( $name ),+ ];
    };
}

methods! {
    // proc — 进程管理
    PROC_SPAWN = "proc.spawn";
    PROC_LIST = "proc.list";
    PROC_INFO = "proc.info";
    PROC_KILL = "proc.kill";
    PROC_SUSPEND = "proc.suspend";
    PROC_RESUME = "proc.resume";
    PROC_SNAPSHOT = "proc.snapshot";
    PROC_RESTORE = "proc.restore";
    // nav — 导航
    NAV_GOTO = "nav.goto";
    NAV_BACK = "nav.back";
    NAV_FORWARD = "nav.forward";
    NAV_RELOAD = "nav.reload";
    // view — 观察
    VIEW_SNAPSHOT = "view.snapshot";
    VIEW_SCREENSHOT = "view.screenshot";
    // act — 操作
    ACT_CLICK = "act.click";
    ACT_TYPE = "act.type";
    ACT_PRESS = "act.press";
    ACT_SCROLL = "act.scroll";
    ACT_SELECT = "act.select";
    ACT_UPLOAD = "act.upload";
    ACT_TAKEOVER_START = "act.takeover.start";
    ACT_TAKEOVER_END = "act.takeover.end";
    /// 接管期间坐标点击（ADR-0010）：仅当调用者是当前 pid 的接管 holder 时可用，
    /// 否则 `E_CAP_DENIED`；不经 takeover_gate 挂起队列（坐标动作无 ref/generation
    /// 过期保护，不能被排队到未来不确定的页面状态下执行）。
    ACT_POINT_CLICK = "act.point.click";
    // dom / js
    DOM_EXTRACT = "dom.extract";
    JS_EXEC = "js.exec";
    // evt — 事件
    EVT_WAIT = "evt.wait";
    EVT_SUBSCRIBE = "evt.subscribe";
    EVT_UNSUBSCRIBE = "evt.unsubscribe";
    // state — State VFS
    STATE_READ = "state.read";
    STATE_WRITE = "state.write";
    STATE_LIST = "state.list";
    STATE_EXPORT = "state.export";
    STATE_IMPORT = "state.import";
    // net — 网络
    NET_RULES_SET = "net.rules.set";
    NET_RULES_GET = "net.rules.get";
    NET_LOG = "net.log";
    // cap — 能力
    CAP_REQUEST = "cap.request";
    CAP_LIST = "cap.list";
    CAP_GRANT = "cap.grant";
    CAP_REVOKE = "cap.revoke";
    CAP_APPROVE = "cap.approve";
    CAP_PENDING = "cap.pending";
    // wf — 工作流
    WF_CREATE = "wf.create";
    WF_LIST = "wf.list";
    WF_RUN = "wf.run";
    WF_CANCEL = "wf.cancel";
    // obs — 观测
    OBS_JOURNAL = "obs.journal";
    OBS_TRACE = "obs.trace";
    OBS_REPLAY_EXPORT = "obs.replay.export";
    // sys
    SYS_INFO = "sys.info";
}

/// 方法名是否属于系统调用表。
pub fn is_known(name: &str) -> bool {
    ALL.contains(&name)
}
