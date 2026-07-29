# 配置

> 文档类型：操作指南
> 状态：Implemented
> 适用范围：Current
> 权威内容：用户目录、界面偏好、模型 Provider 配置和凭据安全约束
> 返回：[文档索引](index.md)

CADX 使用 `~/.cadx` 作为用户级工作目录。Provider 设置只从
`~/.cadx/config.yaml` 读取；endpoint、model、timeout 和 credential
都不会从环境变量读取。

桌面应用首次启动时创建以下目录：

```text
~/.cadx/
  config.yaml
  preferences.yaml
  projects/
```

`projects/Untitled.cadx` 只是新 workspace 的默认保存路径，不会在启动时创建；首次成功
保存后才产生 primary project file。

## 界面语言

桌面界面支持 English 和简体中文。首次启动且尚无 `preferences.yaml` 时，CADX 读取系统
locale：`zh` 及 `zh-*` 使用简体中文，其他或无法识别的 locale 使用 English。顶部工具栏
可以在运行时切换语言；选择会立即生效并写入独立的
`~/.cadx/preferences.yaml`：

```yaml
version: 1
language: simplified-chinese
```

英文值为 `english`。`version` 必须为 `1`，未知字段、未知语言和未知版本都会被拒绝。
语言属于用户偏好，不写入 `.cadx` 工程，也不改变实体名称、分支名、公式、单位、路径、
Provider endpoint 或 model。底层技术错误和 Agent 生成的工程内容可以保留来源语言。
应用随包加载 Apache-2.0 的 CJK fallback font，因此中文界面不依赖操作系统字体配置。

## Provider 配置

`config.yaml` 初始包含空凭据。真实密钥只应写入本机文件，不得加入版本控制：

```yaml
version: 1

provider:
  endpoint: "https://api.openai.com/v1"
  model: "gpt-5.6-luna"
  api_key: "replace-with-your-provider-key"
  timeout_seconds: 45
```

- `version` 必须为 `1`。
- `endpoint`、`model` 和 `api_key` 为必填项。
- `timeout_seconds` 缺省为 `45`，取值必须在 `1..=300` 秒。
- 未知字段会被拒绝，避免拼写错误静默改变行为。

远程 Planner 在展示上下文披露时读取一次配置，并在实际调用前再次读取。
如果授权后修改 endpoint 或 model，已有项目 grant 不再覆盖新的披露，调用会被拒绝。

项目授权不是笼统的“允许联网”。每个持久 grant 精确绑定 project ID、endpoint/model、允许的
数据类别与 capability、project summary 或 selected entity ID 范围、payload 上限、创建时间、
可选到期时间和撤销状态。Workbench 提供 1 小时、24 小时、7 天和“直到撤销”四种有效期，
并可显式撤销当前匹配的 grant。grant 可跨 revision、PromptChangeSet 和 AgentRun 重用，但
不会放宽其绑定范围。

每次实际发送都会从当前 revision 重新构造披露并再次检查 grant，而不是重用旧 payload。
发送事件精确记录 project/grant ID、task、PromptChangeSet、AgentRun、endpoint/model、requested
capability、selected object ID、source revision、数据类别、payload bytes、payload SHA-256 和
发送时间。实际发送的 `RemoteContext` 最大 64 KiB，最多包含 1024 个 selected entity ID，
固定类别为 task goal、document metadata/statistics、selection identifiers 和 granted
capabilities。当前不发送 geometry、attachment 或 source file。当前生成 context schema v3，
其中包含稳定 project ID；迁移工程中的旧 schema audit 只按其原始版本保留验证，不能代替
当前 grant。Desktop 先在主 workspace 中把同一披露作为 `.cadx` format v10 run event 持久化，
再通过线程握手允许后台 worker 调用 Provider；完整 payload 和响应 transcript 不进入工程。

## 文件与凭据安全

在 Unix 上，CADX 使用 `0700` 创建 `~/.cadx` 和 `~/.cadx/projects`，
使用 `0600` 创建 `config.yaml` 和 `preferences.yaml`。应用拒绝这两个配置文件的符号
链接，以及 group 或 other 可访问的配置文件和目录。偏好更新使用同目录私有临时文件、
flush、原子 rename 和 parent-directory sync。

API key 在 `Debug` 输出中始终脱敏，并且不得进入：

- `.cadx` 工程；
- task event 或 semantic history；
- provider disclosure；
- endpoint URL；
- 状态栏、普通日志或崩溃报告。

企业 endpoint allowlist、组织级策略下发和操作系统 credential store 尚未实现。当前配置文件
与项目 grant 边界是已实现行为，目标上下文授权模型见[目标架构](design.md)。
