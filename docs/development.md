# 开发指南

> 文档类型：开发指南
> 状态：Partial
> 适用范围：Current
> 权威内容：本地质量门禁、变更规则和发布检查
> 返回：[文档索引](index.md)

CADX 当前使用 Rust `1.95` 和 Rust 2024 Edition。仓库中的
`rust-toolchain.toml` 固定 CI 使用的编译器和 lint 组件。

## 本地质量门禁

修改工程契约前，运行完整验证：

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release
```

CI 在 Ubuntu、macOS 和 Windows 上运行这些门禁。原生窗口、字体和 GPU driver
不在单元测试覆盖范围内，桌面发布仍需在各支持平台执行 smoke test。

## 变更规则

- 当前模型写入必须保持为经 `TaskWorkspace` 提交的 `CommandTransaction`；
  renderer 和 planner 不得获得可变文档引用。
- 修改 document、workspace、task 或 native archive 的序列化结构时，必须同时添加
  migration 和 fixture。
- 除非通过新 format version 明确修改边界，不得放宽 `.cadx` archive allowlist
  和 payload limit。
- exchange parser 必须有资源上限并位于 `cadx-io`。导入先生成并预检 typed
  transaction；导出只读取 immutable document，报告语义损失并原子替换目标文件。
- provider credential 不得进入源码、endpoint URL、task event、history、native
  project、普通日志或状态文本。
- authorization、recovery、persistence、interchange、geometry 或 provider 行为变化时，
  必须增加聚焦的回归测试。
- recovery 写入不得阻塞 UI thread；退出前必须等待正在进行的 writer，primary save
  成功前不得删除 sidecar。
- remote provider 调用不得阻塞 UI thread；主线程必须先验证未过期且未撤销的 project grant，
  并持久化绑定当前 payload、project/grant 和发送时间的 hash-bound remote-send audit，后台
  收到启动确认后才能操作 workspace clone 和调用 Provider。Worker
  workspace 不得直接安装；主线程只能通过 `KernelFacade` 重放目标 task 的 typed plan，
  保留无关 commit、拒绝对象冲突，并在 base/task 真正过期时丢弃结果。
- project grant 或 remote-send audit 的 wire 变更必须提升 native format，并覆盖 grant ledger
  重放、到期/撤销、跨 revision 重授权、provider 调用前拒绝和审计绑定篡改测试。
- Task action commit 必须同时绑定 task、PromptChangeSet 和 AgentRun；pause/resume 不得创建新
  Run，只有显式 retry 可以创建下一 attempt。任何 wire 变更都必须覆盖旧层级迁移、ID/active
  binding、action 顺序和 commit ownership 篡改拒绝。
- 本地 iterative Planner 每个 decision 最多返回一个 action；成功提交后必须重新观察。可修复
  failure 必须持久化结构化反馈且最多自动修复三次，测试必须覆盖 revision 序列、上限耗尽、
  同对象人工并发、暂停恢复和 execution strategy 篡改拒绝。
- ChangeSet 回滚必须追加补偿而不是移动 branch head；后续修改对象必须保留并进入冲突报告。
  测试必须覆盖无冲突、部分冲突、全部冲突、旧格式迁移和补偿审计篡改拒绝。
- `Current` 文档必须与代码同步；目标设计只能写入 `Target` 文档或 ADR，不能伪装成
  已实现能力。
- 版本、字段、资源上限和格式支持矩阵只在其专题契约中维护，其他文档使用链接引用。

## 文档约定

文档状态和适用范围只以[文档索引](index.md)中的定义为准。产品与工程叙述使用中文，
Rust symbol、命令、路径和 wire field 保留英文；许可证、第三方声明、上游原文和机器生成
清单可以保留其法定或来源语言。规范性要求使用“必须 / 应该 / 可以”。

## 发布基线

发布桌面构建前，必须验证：

- 签名的 packaging pipeline；
- 原生窗口启动和支持的 GPU/device matrix；
- English 和简体中文切换、偏好重启恢复，以及 CJK glyph 非空白渲染；
- save/open/fork/compare，以及完整/带冲突的 Prompt 补偿回滚；
- crash sidecar 的 recover/discard；
- 支持格式的 fixture、Prompt/Run 迁移与 ownership 篡改测试。

签名打包流水线尚未在本仓库中实现。
