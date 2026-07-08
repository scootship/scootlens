# ADR-0004：控制台采用纯 Web，不做桌面 GUI

- 状态：Accepted
- 日期：2026-07-08

## 背景

曾评估 Tauri 2 桌面壳方案。ScootLens 的主要部署形态是服务器/headless 常驻守护进程；
控制台核心界面（实时画面、时间线、检查器、审批）全部是 Web UI 强项。

## 决策

Console 为纯 Web SPA（Svelte 5 + Vite + TS），构建产物由 `scootlensd` 静态托管，浏览器直开。
不做任何桌面壳。

## 备选方案

- Tauri 2 壳 + 同一套 Web UI：多一个交付物与平台矩阵（含 Linux WebKitGTK 怪癖），
  对 headless 部署场景零收益；放弃（未来若有桌面刚需可低成本重启，UI 代码不变）
- egui/iced 原生 GUI：复杂检查器/时间线开发成本数倍；拒绝
- Electron：与资源足迹目标直接矛盾；拒绝

## 后果

- 单一交付物；远程/本地体验一致
- Console 与 Agent 同走 ABI，持续 dogfooding 接口完备性
- 桌面级能力（托盘、全局快捷键）不可用——当前无此需求
