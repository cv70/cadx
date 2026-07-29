# ADR-0005：确定性验证与 Release

> 文档类型：架构决策记录
> 状态：Accepted
> 适用范围：Target
> 权威内容：为何分离 Planner claim、本地 evidence 和 Release sign-off
> 返回：[文档索引](../index.md) · [ADR 索引](README.md)

## 背景

生成模型可能遗漏错误、伪造“验证通过”或使用与本地工程不同的规则。单个 action 几何有效，也不代表整个工程满足制造、材料、交换或组织发布策略。若所有 warning 都阻止日常编辑，设计过程又会无法渐进推进。

## 决定

Planner 和远程服务返回的验证描述统一视为 `UntrustedClaim`。只有本地、锁定版本的 validator 能针对精确 candidate revision 生成 `ValidationEvidence`，并由 `KernelFacade` 用作 action 提交闸门。

结构损坏、悬空引用、量纲错误、约束不收敛、无效 B-rep、过期前置条件和不可豁免规则违反是 hard error，阻止该 action 提交。warning 和 info 可以进入 Draft，但必须绑定对象、规则、测量值和 evidence，不能静默隐藏。

Release 是独立于 action 成功的 sign-off 流程。它固定 revision 和[目标设计](../design.md)定义的完整 `PackLock`，执行发布策略要求的全量再生与验证，并生成由用户或受信设备签署的 `ReleaseAttestation`。修改已发布 revision 会产生新的 Draft，原 attestation 保持不可变。

## 接口影响

- `ValidationEvidence` 必须包含 candidate revision、输入 state hash、validator 标识及版本、Pack lock hash、结构化诊断、测量和 outcome。
- evidence 只对绑定的 candidate 有效，不能移用于其他 revision 或版本锁。
- `ReleaseAttestation` 包含项目、revision、state hash、Pack lock hash、evidence hashes、发布策略、signer 和签名。
- warning 是否允许 Release 由锁定策略决定，Agent 不能降低严重级别或修改门槛。

## 替代方案

- **信任 Planner 返回的通过报告**：拒绝。生成者不能兼任提交信任根。
- **所有 warning 都阻止 action**：拒绝。它不支持渐进设计和有意保留的工程权衡。
- **最后一个 action 成功即视为发布**：拒绝。局部验证不能证明全工程和交换产物满足发布策略。
- **只保存文本日志**：拒绝。无法绑定输入、版本和可机器核验测量。

## 后果

Draft 能保持流畅，制造或交付保证则有独立、可复验的证据边界。成本是本地必须具备完整 validator，Release 需要额外时间，全量检查结果也必须随版本锁管理。

## 安全限制

签名只证明 signer 对指定 hash 和策略作出签署，不证明 validator 本身无缺陷。Release 的可信度取决于本地 Core、锁定 Pack、规则、密钥保护和输入材料数据；未受信原生 Pack 会削弱整个证明链。
