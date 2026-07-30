# CADX

CADX 是使用 Rust 2024 Edition 构建的本地优先 AI-Native CAD desktop application。
它把 CAD 工程视为可编辑的设计 workspace：Agent 观察模型、调用受限工具并提交可重放
变更，人类在同一个模型、历史和分支系统中继续编辑与决策。

当前仓库是可运行的 2D/参数化/机械视图纵切，不是完整 B-rep kernel 或完整 MCAD
产品。当前 3D 只显示 polygonal `SketchProfile -> Extrude`，不执行孔或 boolean；精确
边界见[机械视口](docs/mechanical-viewport.md)。目标架构、当前能力和格式契约从统一文档
入口开始阅读：

- [文档索引](docs/index.md)
- [目标架构](docs/design.md)
- [当前实现与迁移路线](docs/implementation.md)

## 运行

```sh
cargo run -p cadx-app
cargo run -p cadx-app -- --project path/to/model.cadx
```

首次启动会在 `~/.cadx` 创建用户目录、Provider 配置模板和独立出口策略。配置、安全边界和当前支持的
工作流见[配置指南](docs/configuration.md)与[当前实现](docs/implementation.md)。Native app
当前要求系统能初始化 WGPU adapter，没有 Glow 或 CPU startup fallback。

桌面界面支持 English 与简体中文，默认跟随系统 locale，也可从顶部工具栏即时切换；
选择保存在独立的 `~/.cadx/preferences.yaml` 中。
远程 Planner 只能在项目级访问授权覆盖当前 endpoint、model、数据类别、能力、对象范围和
payload 上限时发送；授权可设置到期时间并随工程持久化或显式撤销，每次发送仍记录精确的
revision、payload 字节数与 SHA-256 审计。每次发送还必须通过默认拒绝的
`~/.cadx/egress-policy.yaml` 本机 endpoint/model allowlist。
Prompt 历史支持冲突感知补偿回滚：后续人工或其他 Agent 已修改的对象会保留并明确报告，
未冲突对象通过新的可审计提交恢复。
本地 Planner 按 `observe -> action -> validate -> commit -> re-observe` 循环执行；本地验证或
对象版本冲突会形成结构化反馈并最多自动修复三次，已成功提交的 action 不会被后续失败抹去。
远程 Planner 使用相同的逐轮提交边界：每次 Provider 调用前都重新观察、重新校验项目授权并
持久化精确发送审计；Provider 每轮只能返回一个 action 或 complete，跨轮总预算随工程保存。

## 验证

完整验证命令和开发规则见[开发指南](docs/development.md)。

## 许可证

CADX 使用 MIT License，详见 [LICENSE](LICENSE)。

第三方依赖声明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
