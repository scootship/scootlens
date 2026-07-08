# ADR-0003：HAL 语义对齐 WebDriver BiDi

- 状态：Accepted
- 日期：2026-07-08

## 背景

多引擎是核心差异化（Chromium/WPE/Servo），但各引擎自动化协议差异巨大（CDP 私有 vs WebKit
接口）。HAL trait 若以 CDP 概念建模，WPE/Servo 驱动将被迫模拟 Chromium 私有语义。

## 决策

`scootlens-hal` trait 的语义以 W3C **WebDriver BiDi** 标准为参照系建模（导航、脚本、输入、
网络拦截等 module 概念），配合能力矩阵允许驱动部分实现（`E_UNSUPPORTED`）。

## 备选方案

- 以 CDP 为事实标准建模：短期最快，长期把多引擎承诺做死；拒绝
- 完全自创抽象：失去标准演进红利与生态互操作可能；拒绝

## 后果

- Chromium 驱动需做 CDP→BiDi 语义映射（成本可控，映射关系文档化）
- WebKit 阵营对 BiDi 的官方支持是长期利好
- conformance suite 直接按 BiDi 语义编写，跨驱动复用
