//! # scootlens-hal
//!
//! 引擎硬件抽象层：内核只依赖本 crate 的 trait，驱动在二进制层注入。
//! 语义对齐 WebDriver BiDi（ADR-0003）；能力矩阵允许部分实现（`E_UNSUPPORTED`）。
//!
//! `conformance` 模块提供跨驱动一致性测试套件（docs/05-engine-hal.md）。

pub mod conformance;
mod driver;
mod types;

pub use driver::{EngineDriver, EngineHandle, HalResult};
pub use types::{
    A11yNode, A11ySnapshot, ActResult, EngineCaps, EngineEvent, EngineMetrics, HistoryDir,
    InputAction, NavResult, ProfileSpec, SnapshotOpts, StateBundle,
};
