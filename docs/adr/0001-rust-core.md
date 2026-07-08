# ADR-0001：核心语言采用 Rust

- 状态：Accepted
- 日期：2026-07-08

## 背景

ScootLens 是常驻内核型系统（Web OS），对性能、内存足迹与安全（内存安全、并发安全）要求高；
需要与 C/C++ 引擎（WebKit）做 FFI；未来 Servo 引擎为纯 Rust。

## 决策

内核、驱动、网关、CLI 全部使用 Rust（stable toolchain），异步运行时 tokio。

## 备选方案

- Go：GC 停顿与内存足迹更大；FFI 体验差；拒绝
- Node/TypeScript：与"去掉 Playwright 的 Node 层开销"的动机矛盾；拒绝
- C++：无内存安全保证，安全铁律不允许；拒绝

## 后果

- 开发速度前期较慢，换取长期运行稳定性与安全性
- 除 FFI 驱动外全线 `#![forbid(unsafe_code)]` 成为可能
- 与 Servo 生态天然对齐
