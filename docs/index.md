# CADX 文档索引

> 文档类型：导航
> 状态：Accepted
> 适用范围：Target / Current / Roadmap
> 权威内容：文档分类、阅读顺序和单一事实来源规则

CADX 是本地优先的 AI-Native CAD 平台。目标产品覆盖设计创作型 MCAD，当前仓库提供
2D drafting、基础参数/约束、派生 extrusion viewer、semantic history、native
persistence 和受限 Agent runtime 的可运行纵切。

## 推荐阅读顺序

1. [目标架构](design.md)：产品边界、核心不变量、目标组件和数据流。
2. [当前实现与迁移路线](implementation.md)：当前代码已经做到什么、还缺什么。
3. [架构决策记录](adr/README.md)：为什么选择这些边界，以及接受了哪些后果。
4. 根据工作内容阅读下面的专题契约或指南。

## 专题文档

### 操作与开发

- [配置](configuration.md)：用户目录、Provider YAML、出口策略和凭据约束。
- [开发指南](development.md)：构建、测试、变更规则和发布门禁。

### 当前格式与交换契约

- [`.cadx` 原生工程格式](native-project-format.md)：当前 archive、schema migration、
  save/load 和 recovery sidecar。
- [DXF 交换契约](dxf-interchange.md)：当前 2D import/export 支持矩阵和有损语义。
- [PDF 2D 视图导出](pdf-export.md)：当前单页 vector projection 和输出边界。

### 当前组件契约

- [本地验证证据](validation-evidence.md)：candidate commit gate、state hash、claim 分离和限制。
- [机械视口](mechanical-viewport.md)：当前 extrusion mesh、3D 交互和限制。

## 文档状态

| 状态 | 含义 |
| --- | --- |
| `Proposed` | 尚在讨论，不能作为实现依据。 |
| `Accepted` | 决策已经确认，但不代表代码已经实现。 |
| `Implemented` | 当前代码和测试已经支持。 |
| `Partial` | 已有可运行纵切，但目标契约尚不完整。 |
| `Planned` | 尚未实现。 |
| `Superseded` | 已由新的 ADR 或规范替代，仅保留用于追溯。 |
| `Deprecated` | 仅为兼容历史保留。 |

适用范围使用：

- `Target`：目标产品或架构契约。
- `Current`：可由当前仓库代码验证的事实。
- `Roadmap`：从 Current 迁移到 Target 的顺序。

## 单一事实来源

- 产品定位、目标不变量和目标接口只在[目标架构](design.md)定义。
- 当前 crate、能力状态、已知缺口和迁移阶段只在
  [当前实现与迁移路线](implementation.md)维护。
- 架构取舍和替代方案只在 [ADR](adr/README.md)维护。
- Format version、schema field、resource limit 和支持矩阵只在对应专题契约维护。
- 配置方法和完整开发门禁只在操作与开发指南维护。
- 根 `README.md` 只承担仓库入口，不复制上述规范。

当 Target 和 Current 不一致时，这表示尚未完成的迁移，不应通过改写当前事实来消除差异。
