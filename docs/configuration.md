# 配置

> 文档类型：操作指南
> 状态：Implemented
> 适用范围：Current
> 权威内容：用户目录、界面偏好、模型 Provider 配置、出口策略和凭据安全约束
> 返回：[文档索引](index.md)

CADX 使用 `~/.cadx` 作为用户级工作目录。Provider 设置只从
`~/.cadx/config.yaml` 读取；endpoint、model、timeout 和 credential
都不会从环境变量读取。独立的 `~/.cadx/egress-policy.yaml` 决定哪些
endpoint/model 可以真正发起网络请求，`config.yaml` 不能自行扩大该范围。

桌面应用首次启动时创建以下目录：

```text
~/.cadx/
  config.yaml
  egress-policy.yaml
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

远程 Planner 创建时读取配置，并把该 endpoint/model 绑定到披露、grant 检查和 Run identity。
下一次创建 Planner 时若 endpoint 或 model 已修改，已有项目 grant 不再覆盖新的披露，调用会被拒绝。

## Provider 出口策略

`egress-policy.yaml` 是独立、默认拒绝的本机网络出口边界。首次启动生成的模板只允许默认
OpenAI endpoint 和默认 model：

```yaml
version: 1

allowed_providers:
  - endpoint: "https://api.openai.com/v1"
    models:
      - "gpt-5.6-luna"
```

每条授权是精确的 endpoint/model 元组，不支持 host、path 或 model 通配符。空的
`allowed_providers: []` 会拒绝所有远程请求。若使用其他 Provider 或 model，必须分别修改
`config.yaml` 和 `egress-policy.yaml`；只修改前者不会获得联网权限。

比较前会规范化 scheme、DNS host 大小写、默认端口和末尾 `/`，因此
`https://EXAMPLE.com:443/v1/` 与 `https://example.com/v1` 等价。scheme、非默认端口和路径
仍精确绑定。endpoint 必须使用 HTTPS；只有 `localhost`、`127.0.0.1` 和 `::1` 可以使用
HTTP。userinfo、query、fragment、百分号编码路径、控制字符、空 model、model 首尾空白、
重复规则、未知字段和未知版本均被拒绝。策略最多 64 KiB、128 个 endpoint/model 元组。

TaskAgent 在生成持久发送审计前检查一次策略，消耗一次性发送轮次前再检查一次；真实 GenAI
adapter 在 HTTP 入口还会重新加载并检查。因此策略删除、变为无效或撤销规则后，已存在的
项目 grant 也不能继续发送。若策略恰在审计之后变化，本轮会合法终结为失败，但不会调用
Provider；下一轮不会复用该审计。

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
capabilities，以及 execution state。Execution state 包含当前 action index、持久化 action/decision
总预算及剩余额度，并在存在可修复失败时包含结构化反馈。当前不发送 geometry、attachment 或
source file。

当前生成 context schema v4，其中包含稳定 project ID 和上述 execution state。远程执行严格按
轮进行：主线程重新观察当前 revision、重新检查 grant，并先把相同 payload 的 hash-bound
`ProviderDisclosure` 写入 `.cadx` format v12 run event；随后一次性授权对象才允许后台 worker
调用 Provider。Provider 每轮只能返回一个 `action` 或 `complete` decision；action 在主线程
基于被观察的 snapshot 转换为本地 typed transaction，经本地权限、对象前置条件和 validator
后提交，下一轮再重新观察。可修复 rejection 会进入下一轮 payload。完整 payload、Provider
响应 transcript 和 credential 不进入工程。迁移工程中的旧 schema audit 只按其原始版本保留
验证，不能代替当前 grant 或当前逐轮审计。

## 文件与凭据安全

在 Unix 上，CADX 使用 `0700` 创建 `~/.cadx` 和 `~/.cadx/projects`，
使用 `0600` 创建 `config.yaml`、`egress-policy.yaml` 和 `preferences.yaml`。应用拒绝这些
文件的符号链接、非普通文件，以及 group 或 other 可访问的配置文件和目录。读取时还会比较
检查对象与实际打开文件的 identity，拒绝检查期间发生的路径替换。偏好更新使用同目录私有临时文件、
flush、原子 rename 和 parent-directory sync。

API key 在 `Debug` 输出中始终脱敏，并且不得进入：

- `.cadx` 工程；
- task event 或 semantic history；
- provider disclosure；
- endpoint URL；
- 状态栏、普通日志或崩溃报告。

当前已实现本机强制 endpoint/model allowlist；它不是带签名的组织策略，拥有当前 OS 用户权限的
操作者仍可修改该文件。组织级签名策略下发、集中策略管理和操作系统 credential store 尚未实现。
当前配置、出口策略与项目 grant 边界见[目标架构](design.md)。
