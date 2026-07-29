# CADX 架构决策记录

> 文档类型：ADR 索引
> 状态：Accepted
> 适用范围：Target
> 权威内容：决策编号、状态和导航
> 返回：[文档索引](../index.md) · [目标设计](../design.md)

## 用法

ADR 记录已经作出的高影响、难逆转架构决定，以及选择它们的原因和后果。目标接口的完整行为以 [目标设计](../design.md) 为准；当前实现状态以 [实现说明](../implementation.md) 为准。

状态约定：

- `Proposed`：仍在讨论，不能作为实现依据。
- `Accepted`：已接受，后续实现和文档必须遵守。
- `Superseded`：已被新 ADR 替代，保留用于历史追溯。
- `Deprecated`：不再推荐，但尚未被单一新决定替代。

修改已接受决定时，不直接重写其结论；应新增 ADR，并在旧记录中标注替代关系。纯文字澄清可以原地修改，但不得改变决定的语义。

## 已接受决策

| ADR | 决策 | 状态 |
| --- | --- | --- |
| [0001](0001-product-scope-and-delivery.md) | 产品范围与分阶段交付 | Accepted |
| [0002](0002-local-authority-and-cloud-sync.md) | 本地权威与云端同步 | Accepted |
| [0003](0003-typed-core-and-domain-pack.md) | 强类型核心与 Domain Pack | Accepted |
| [0004](0004-prompt-changeset-and-revert.md) | PromptChangeSet 与补偿回滚 | Accepted |
| [0005](0005-validation-and-release.md) | 确定性验证与 Release | Accepted |
| [0006](0006-agent-context-and-semantic-tools.md) | Agent 上下文与语义工具 | Accepted |
| [0007](0007-storage-replay-and-audit.md) | 存储、重放与审计 | Accepted |
