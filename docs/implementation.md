# CADX 当前实现与迁移路线

> 文档类型：实现状态
> 状态：Partial
> 适用范围：Current / Roadmap
> 权威内容：当前仓库事实、目标差距和迁移阶段
> 返回：[文档索引](index.md)

本文描述当前代码已经做到什么，以及它与[目标架构](design.md)之间还缺少什么。
本文不是目标架构的替代品，也不复制文件格式、交换格式和组件专题中的精确字段与限制。

## 1. 当前产品纵切

CADX 当前是本地优先的 Rust 2024 desktop prototype。用户可以创建 task，让本地或远程
Planner 产生受限 action，并通过 typed transaction 修改可编辑模型。Human edit 和
Agent edit 进入同一 semantic history。

当前纵切覆盖：

- 基础 2D drafting 和 layer 管理；
- parameter formula 与小型 2D constraint solver；
- `SketchProfile` / `Extrude` 的派生 3D mesh viewer；
- task authority、action checkpoint 和 semantic commit；
- revision-bound immutable document snapshot、`PreparedAction`、对象版本 precondition 和
  幂等 action retry；
- 绑定 candidate state hash 的 core-produced validation evidence；
- native project、crash recovery、DXF exchange 和 PDF 2D view export；
- deterministic demo Planner 和受 project grant/budget 限制的远程 Planner。

它尚不是完整参数化 MCAD、B-rep kernel、Domain Pack 平台或生产级 agentic runtime。

## 2. Workspace 与 Crate 边界

```text
cadx/
  crates/
    cadx-config/     # 用户目录和 Provider YAML 配置
    cadx-core/       # Document、参数、约束、typed command、task、history
    cadx-agent/      # Planner contract、远程 adapter 和 task runner
    cadx-io/         # Native archive、migration、DXF/PDF 边界
    cadx-render/     # Immutable 2D/3D scene、camera、picking、snapping
    cadx-app/        # eframe/egui desktop workbench
  docs/
```

当前依赖方向的重要约束：

- `cadx-core` 不依赖 AI、renderer 或 window system。
- `cadx-render` 读取 immutable `CadDocument` 并生成 derived scene，不写模型。
- `cadx-agent` 是唯一依赖 `rust-genai` 和 Tokio 的 crate。
- `cadx-io` 负责 bounded parser、migration 和原子文件替换。
- `cadx-config` 不依赖 app 或 agent runtime。

## 3. 能力状态

| 子系统 | 状态 | 当前边界 |
| --- | --- | --- |
| Typed document 与 transaction | Implemented | `CommandTransaction` 在 document clone 上预检，成功后整体替换。 |
| Layer、基础 entity 与 reference validation | Implemented | `EntityKind` 和 `Domain` 是 core 中的封闭 enum。 |
| Parameter expression | Partial | 支持本地算术、单位转换和 cycle rejection；尚无通用带量纲 `Quantity`。 |
| 2D sketch constraint | Partial | 支持 coincidence、horizontal、vertical、distance、radius；不是生产级通用 solver。 |
| Semantic history、branch、undo/redo | Implemented | 单 parent commit、snapshot + forward replay；没有 semantic merge。 |
| Native persistence 与 recovery | Implemented | 已实现 lossless project archive 和 recovery sidecar；详见[原生工程格式](native-project-format.md)。 |
| Task/Prompt/Run 与 pause/resume | Implemented | DesignTask 跨 Prompt；ChangeSet 保存授权/诊断，Run 保存 checkpoint/identity/commit 顺序并可跨无关 commit 恢复；Capability 粒度仍较粗。 |
| Agent runtime | Partial | 本地 `TaskPlanner` 已逐 action re-observe/replan，并对可修复失败最多重试三次；远程 `RemoteTaskPlanner` 仍是 grant 覆盖下的单次批计划，尚未进入逐 action 多轮。 |
| Remote Planner | Partial | 已有不可越权 `RemoteContext`、持久 project grant、expiry/revocation、每次发送的 hash-bound audit、budget、strict DSL 和非阻塞 UI worker；尚无企业 endpoint allowlist、组织策略或 OS credential store。 |
| 2D viewport | Implemented | CPU picking/snapping、基础 authoring 和 egui painter。 |
| Mechanical viewport | Partial | bounded extrusion mesh 已通过 `wgpu` 提交；详见[机械视口](mechanical-viewport.md)。 |
| DXF exchange | Implemented | 有界、有损的 2D 子集；详见[DXF 交换契约](dxf-interchange.md)。 |
| PDF 2D view export | Implemented | 有界的单页 2D vector projection；详见[PDF 2D 视图导出](pdf-export.md)。 |
| Domain Pack | Planned | 没有 plugin ABI、typed pack payload、pack migration 或 pack lock。 |
| Kernel-owned validation evidence | Partial | core 已对 candidate structure 和小型 constraint solver 生成 state-bound evidence；尚无 Pack/B-rep/rule evidence。 |
| Prepared action 与对象并发 | Partial | 已有本地 prepare、typed object precondition、ABA tombstone 和 idempotency key；对象 ID 仍为 document-local `u64`，尚无 Pack operation/read-set contract。 |
| `PromptChangeSet` 与补偿回滚 | Partial | Prompt/Run 归组和 conflict-aware compensating revert 已实现；补偿保留后续对象写入并记录冲突，但 task write authority 尚不可撤销，也没有 Pack lock gate。 |
| B-rep、stable topology 与 STEP | Planned | 当前没有 Open CASCADE adapter 或 feature regeneration graph。 |
| Cloud event replication | Planned | 没有 CAS push、outbox/inbox 或 remote branch protocol。 |

## 4. Core Mutation Contract

当前 `CadDocument` 保存 schema version、metadata、display unit、layer、entity、parameter
和 sketch constraint。ID 使用 document-local `u64`，geometry 和 parameter value 主要使用
`f64`。

```rust
pub struct CadDocument {
    pub schema_version: u32,
    pub metadata: DocumentMetadata,
    pub units: Units,
    pub layers: BTreeMap<LayerId, Layer>,
    pub entities: BTreeMap<EntityId, Entity>,
    pub parameters: BTreeMap<ParameterId, Parameter>,
    pub constraints: BTreeMap<ConstraintId, SketchConstraint>,
}
```

模型 mutation 当前统一表达为 `CadCommand`：

```rust
pub enum CadCommand {
    CreateLayer { layer: Layer },
    UpdateLayer { layer: Layer },
    DeleteLayer { id: LayerId },
    CreateEntity { entity: Entity },
    UpdateEntity { entity: Entity },
    DeleteEntity { id: EntityId },
    SetParameter { parameter: Parameter },
    DeleteParameter { id: ParameterId },
    CreateConstraint { constraint: SketchConstraint },
    UpdateConstraint { constraint: SketchConstraint },
    DeleteConstraint { id: ConstraintId },
}
```

`CommandTransaction::preview` 逐条模拟 command，并在临时 document 上执行最终结构验证；
`apply` 只有在完整 transaction 成功后才替换真实 document。Human tool、import adapter
和 Agent action 都走这条路径。私有 `DocumentStore` 持有 active document 与 semantic history，
并负责 commit、undo/redo、branch checkout 和文档安装；它的字段不会暴露给其他 Core 模块。
`TaskWorkspace` 的 store 与 task map 对 Core 外私有，只暴露只读 accessor；公开
`KernelFacade` 是 workspace 的唯一可变入口。`DocumentSnapshot` 持有不可变 document 和
创建时的 revision。普通 Human transaction 必须携带 expected revision；显式 prepare 路径则
从 snapshot 生成不可反序列化的短期 `PreparedAction`，保存 input/candidate hash、transaction
涉及对象的版本 precondition 和 idempotency key。Commit 要求 prepare base 位于当前 ancestry，
但允许其间存在不触及这些对象的 commit；同对象、依赖对象和 delete/recreate ABA 会明确冲突。
Task execution 保存 base/expected revision、显式 batch/iterative strategy 与下一 action
preparation，只有 facade 驱动的内部 task commit 原语可以写 task-sourced commit；
`apply_next_task_action` 在提交成功后原子推进 checkpoint 和 expected revision。Iterative strategy
随后回到等待 Planner 状态，下一 action 只能从最新 snapshot 单独 prepare。相同 action 在当前
ancestry 上重试返回原 commit，不会产生重复 geometry。

这是目标存储边界的第三个纵切。独立使用的 `CadDocument` 仍公开 mutation-critical map，
当前 snapshot 也尚未绑定 `PackLock`；object identity 仍是 typed document-local `u64`，对象集合
由 compatibility `CadCommand` 推导，而不是由 Pack `SemanticOperation` 显式声明。SQLite/WAL、
global stable ID、Pack-aware validation 和跨进程单写者协调尚未实现。

## 5. Task 与 Agent Runtime

`DesignTask` 当前是跨多次 Prompt 的长期目标。每个 Prompt 创建独立 `PromptChangeSet`，保存
原始 Prompt、`StructuredGoal`、授权快照、状态、诊断和一个或多个 `AgentRun`；每个 Run 保存
identity、attempt、event、有序 action commit 和可持久化 `TaskExecution`。显式 retry 在同一
ChangeSet 下创建新 Run，pause/resume 和崩溃恢复继续原 Run，已经提交的 action 不会因后续失败
而删除。`TaskAuthority::DirectWrite` 按 `Drafting`、`Mechanical`、
`Architecture`、`Parameters`、`Import` 等 capability 检查 transaction；
`ReviewOnly` 不能写入。

终态 ChangeSet 可以通过 `KernelFacade::revert_change_set` 请求回滚。Core 从目标 action 的 parent
snapshot 重建每个写对象的基线，在请求 revision 上比较对象 tombstone/version：目标 action 后
未被修改的对象进入新的补偿 transaction，后续人工或其他 Agent 已写入的对象保持原样并进入
结构化 conflict report。依赖校验无法安全拆分的对象也保守地作为冲突保留。补偿使用原
ChangeSet 的 authorization snapshot，仍经过普通 prepare、capability、candidate validation、
evidence 和 commit gate；目标 commit 不删除，branch head 不向过去移动。请求只在目标 commit
位于 active ancestry 时接受，并且同一 ChangeSet 当前只允许补偿一次。远程访问 grant 的撤销
不会撤销既有 `TaskAuthority`；task write authority revocation 与 Pack-version gate 尚未实现，
不能把 authorization snapshot 解释为完整企业策略。

本地 `TaskPlanner` 当前执行过程是：

```text
observe latest revision-bound snapshot
  -> planner returns Action(TaskAction) or Complete
  -> locally prepare one action and persist its preparation
  -> validate authority + ancestry + object preconditions + candidate
  -> commit one action and append kernel-produced evidence
  -> advance checkpoint and re-observe latest revision
  -> feed repairable rejection back to planner / continue / pause / complete
```

`PlanningDecision` 一次只能提交一个 action。`Reobserved` 记录 revision、action index 和实体数；
`ActionFailureFeedback` 记录观察 revision、失败类别、intent、tool 和修复次数。Command rejection、
candidate validation failure、stale revision 与 object precondition conflict 会重新观察并反馈；
初始 proposal 失败后最多自动尝试三个修复 proposal，第四次失败将 Run 标记为 Failed 并保留
已成功 action。Action budget 可以在“等待下一次规划”边界暂停，保存/崩溃恢复继续同一个 Run。

Remote Planner 尚未进入该多轮循环。它在 project grant 覆盖的当前 source revision 上生成
bounded action list，之后只在本地逐个提交。grant 可以跨 revision、PromptChangeSet 和 Run
继续授权相同范围，但每次 Provider 发送都会重新构造 disclosure 并追加精确审计。要让远程
provider 在每个 action 后重规划，仍需实现多轮调度、逐轮失败反馈和跨轮总预算。

### Remote Planner

当前 remote path 已实现：

- 本地 `AgentObservation` 只持有 revision-bound `DocumentSnapshot`，再从中构造私有、不可变的
  `RemoteContext`；remote trait 无法取得完整 document；
- 绑定 source revision、固定数据类别、64 KiB payload 和最多 1024 selection ID 的限制；
- stable project UUID，以及绑定 project、endpoint/model、数据类别、capability、对象范围、
  payload 上限和有效期的持久 project grant；
- grant/revoke append-only policy ledger，以及到期或撤销后在 Provider 调用前拒绝；
- 每次发送绑定 task/ChangeSet/Run、source revision、project/grant、categories、bytes、
  SHA-256 和发送时间的 disclosure audit；
- HTTPS 要求和 loopback HTTP 例外；
- action budget；
- OpenAI Responses-compatible adapter；
- 最大 256 KiB、64 action 的 strict remote-plan JSON DSL；
- 完整 document 只在本地 plan materialization、simulation 和 typed transaction validation 使用；
- 主线程通过启动握手，在允许 worker 调用 Provider 前持久化 hash-bound remote-send audit；
- credential 与 project/history 隔离。

`RemoteTaskPlanner` 只能声明 selection request 并接收上述 DTO，网络 adapter 直接发送经过
哈希和审批的 JSON，不能通过 trait 接触 `CadDocument`。Desktop 把 provider 调用放在后台
worker，但 worker workspace 不会直接替换主 workspace。Worker 无持久副作用时只报告结果；
否则 desktop 从 worker 提取 disclosure 和 typed action plan，再通过主 workspace 的
`KernelFacade` 重放。无关人工 commit 会保留，触及同一对象时 task 失败且人工 commit 保留；
base 不再位于 ancestry 或目标 task 已变化时才丢弃 stale result。Paused plan 已经是本地 typed
action，恢复不再调用 Provider，也不需要再次验证远程 grant。已审计 worker 若异常退出且
目标 task 未被用户修改，主 workspace 将 task 标记 Failed，而不是遗留无 plan 的 Running 状态。

当前 grant 与事件台账随 `.cadx` 持久化，grant 可到期或撤销，并且远程发送没有绕过 grant 的
公开 Agent API。企业 endpoint allowlist、组织级签名策略、操作系统 credential store 和逐
action 远程多轮仍未实现。因此 Remote Planner 仍标为 Partial，不能把当前项目级边界解释为
完整的企业授权系统。
配置与 credential 细节见[配置](configuration.md)。

## 6. 验证边界

`TaskAction.validation` 和 `SemanticCommit.validation` 当前保留为 caller/Planner claim，
只用于审计，不参与 commit admission。`History::commit` 在 candidate clone 上原子应用
transaction 后，由 `cadx.core.candidate@1` 生成私有 `ValidationEvidence`。Evidence 绑定
canonical JSON v1 编码的完整 candidate document SHA-256；编码通过 64 MiB bounded writer，
失败或超限不会产生 commit。精确契约见[本地验证证据](validation-evidence.md)。

因此当前代码已经具备的是：

- command shape、reference、finite value、layer lock 等本地结构检查；
- 每次 candidate 自动运行小型 constraint solver；不收敛是 hard failure，存在尚未应用的
  solver update 是 warning；
- 本地 evidence 的 validator id/version、structured checks 和 candidate state hash；
- load/save 与 history replay 重新计算 evidence，并拒绝当前格式中的缺失或篡改。

当前代码尚未具备的是：

- OCCT/Mechanical Pack 生成的 B-rep、拓扑和制造规则 evidence；
- 完整工业 solver/domain rule gate 与 object-scoped diagnostic/measurement；
- Pack lock、rule set、candidate revision 和 evidence hash chain；
- Draft/Release 分离和 signed Release attestation。

当前 SHA-256 只绑定 state 和本地重验结果，没有 signer，不是来源认证或 Release 证明。
在目标能力完成前，文档和 UI 不得声称 Draft 经过完整 B-rep、制造或发布验证。

## 7. History、Persistence 与 Recovery

当前每个 successful transaction 生成一个 `SemanticCommit`，保存 parent、task/ChangeSet/Run
来源、intent、
transaction、diff、untrusted `ValidationReport` claim、local `ValidationEvidence` 和原始
`PreparedActionRecord`。History
使用 immutable forward transaction、
周期 snapshot、branch head 和 branch-local redo stack。Undo 移动 active branch head，
不会删除 commit。

当前已有 conflict-aware compensating revert：完整回滚和带冲突回滚都会创建关联的补偿
ChangeSet；无冲突对象通过新的 ActionCommit 追加，全部对象冲突时只保存补偿结果而不制造空
commit。Loader 会重新核对目标 ancestry、对象基线、request revision、冲突 revision、提交来源
和双向 ChangeSet link。当前仍没有：

- two-parent merge commit；
- 跨 replica 的 global object ID；
- cryptographic audit chain。

Native save/open、migration 和 recovery 的精确契约见
[`.cadx` 原生工程格式](native-project-format.md)。目标 SQLite/WAL 工作库、内容寻址 blob
和 `.cadx` 可移植封包属于 Roadmap，不是当前格式事实。

当前 `.cadx` format v10 持久化 task/PromptChangeSet/AgentRun 层级、三层 commit 来源、
run-bound idempotency key、commit preparation，以及 run execution 的 base/expected revision
和下一 action preparation。v8 保存补偿目标、request revision、恢复对象、冲突与补偿 commit；
v9 增加显式 batch/iterative strategy、逐 action observation、planning completion 和 repair feedback，
并为可逆参数创建增加 `DeleteParameter` diff；v10 增加 stable project UUID、持久 remote grant
台账和绑定 project/grant/发送时间的 context schema v3 audit。Loader 为 v0-v4 推导 legacy execution revision，为 v0-v5 推导
commit/pending-action preparation，并把 v0-v6 legacy task 无损迁移为一个初始 ChangeSet/Run。
Loader 为 v0-v9 生成新 project UUID 和空 remote policy，不会伪造旧工程不存在的 grant。
当前格式缺失或篡改层级 binding、commit ownership、preparation、对象 precondition、hash、
idempotency key 或 checkpoint 都会拒绝。Run output 之间允许穿插无关 commit，但必须保持
ancestry 和 action index 顺序；对象冲突不会静默覆盖。

## 8. Workbench 与 Rendering

当前 desktop workbench 提供：

- English / 简体中文运行时切换、系统 locale 默认值和独立私有偏好持久化；
- task 创建、同一 task 追加 Prompt、ChangeSet retry/cancel、direct/review authority、run-next、
  pause/resume、conflict-aware revert、Prompt/Run 层级和 event log；
- 2D pan/zoom/fit、CPU picking、geometry/grid snapping；
- line、rectangle、circle、arc 和 aligned dimension authoring；
- layer visibility、locking、color、rename、reassign 和 atomic delete；
- parameter/formula editing 和 constraint diagnostics；
- history compare、branch open、undo/redo；
- native save/open、recovery decision、DXF 和 PDF controls；
- bounded 3D extrusion scene、orbit、fit、GPU depth/shading 和 CPU solid picking。

Rendering 只消费 immutable document-derived data。2D 当前由 egui painter 绘制；3D 将
CPU 派生的 extrusion mesh 转换为 GPU buffer，通过 `eframe` 的 `wgpu` callback、深度缓冲
和 face/edge pipeline 提交。Off-screen GPU picking、device-loss recovery、production
B-rep tessellation、LOD 和 drawing-view generation 尚未实现。

egui 默认 Latin font 之后注册随包的 Droid Sans Fallback，覆盖简体中文 glyph。界面文本
由 `UiLanguage` 在渲染或状态生成时选择；语言偏好不进入 document、history 或 task event。
固定的 CAD 标识、数值、单位、路径和 provider/model 名称不翻译。

Native app 当前强制选择 WGPU renderer；adapter 或 surface 初始化失败时没有 Glow/CPU
startup fallback。macOS/Metal 已做原生交互 smoke，Windows/Linux 当前只有 CI build 和
unit test，尚无真实 adapter、surface 或 pixel coverage。精确平台与性能边界见
[机械视口](mechanical-viewport.md)。

## 9. Current 到 Target 类型迁移

目标类型不会在现有 API 上直接改名；涉及信任边界的类型必须拆分：

| Current | Target | 迁移关系 |
| --- | --- | --- |
| `TaskWorkspace` + `CadDocument` | `DocumentStore` + `DocumentSnapshot` | 私有 store 已持有 workspace document/history，`KernelFacade` 是公开唯一写入口，并提供 revision-bound snapshot；standalone document map、Pack lock 和 durable store 仍待迁移。 |
| `TaskAction` + core `PreparedAction` | `SemanticOperation` + Pack-prepared `PreparedAction` | Core 已产生不可远程反序列化、带对象前置条件/hash/幂等键的 candidate；Planner 仍直接返回 compatibility `CadCommand` transaction，尚未拆出 Pack-owned operation。 |
| `SemanticCommit` | `ActionCommit` | 已保存 preparation、对象前置条件、本地 evidence 和 task/ChangeSet/Run 来源；仍缺 Pack lock、Pack operation 和 hash chain。 |
| `DesignTask` | `DesignTask` | 保留为长期目标；不再等同于一次 Prompt 或一次执行。 |
| `TaskExecution` | `PromptChangeSet` + `AgentRun` | 每次 Prompt 独立归组，一个 ChangeSet 可包含可恢复或重试的运行实例。 |
| `TaskAuthority` | `CapabilityToken` | 从粗粒度 enum 迁移为绑定 Task/ChangeSet、Pack、操作和对象范围的授权。 |
| `AgentObservation` | `DocumentSnapshot` query + `RemoteContext` | 本地观察与实际外发 DTO 分离，外发内容绑定授权、revision 和 payload hash。 |
| `ValidationReport` + core `ValidationEvidence` | Pack-aware `UntrustedClaim` + `ValidationEvidence` | 调用方描述已降级为 claim；当前只有 core validator 能生成 structure/constraint evidence，后续扩展 Pack/rule binding。 |

Current 已有可持久化 `PromptChangeSet`、`AgentRun` 和补偿回滚，但 Pack operation、全局稳定 ID
仍未完成；core `PreparedAction` 和 `ValidationEvidence` 也只完成目标契约的 compatibility
阶段，不能只通过同名声称 Pack ABI、Pack lock、B-rep、Release 或签名边界已经完成。

## 10. 迁移路线

### Phase 1：契约与信任边界

- 固定上述类型拆分、Domain Pack ABI、operation schema、evidence 和 Pack lock 契约。
- 已将 Planner claim 与 kernel-produced evidence 分离。
- 已将 remote contract 限制为 hash-bound `RemoteContext`，并实现持久 project grant、期限、
  撤销和逐次发送审计；企业 allowlist 与组织策略仍待实现。
- 已把网络调用移出 UI thread。

### Phase 2：内核与存储

- 已完成第三个纵切：Workspace map 对 Core 外私有、Agent 使用 immutable revision-bound
  snapshot、私有 `DocumentStore` 负责 document/history 状态安装、公开 `KernelFacade` 是唯一
  写入口，并已加入 `PreparedAction`、对象版本 precondition、ABA tombstone 和幂等 retry。
- 继续把 standalone model map 私有化，并把 facade 从 compatibility `CadCommand` 推导对象集合
  扩展为 Pack-owned `SemanticOperation`、显式 read/write set 与 Pack-aware validation。
- 将 object identity 迁移到 global stable ID。
- 引入带量纲 `Quantity` 和规范化 geometry unit。
- 实现 native Domain Pack host、typed payload、validator、migration 和完整 Pack lock。
- 将现有 drafting/mechanical entity 迁移为 compatibility Pack。
- 引入 SQLite/WAL working store、内容寻址 blob、hash chain 和可移植 `.cadx` packaging。

### Phase 3：迭代式 Agent Runtime

- 已落地 `DesignTask -> PromptChangeSet -> AgentRun -> ActionCommit*` 层级，并把授权快照、
  run identity、诊断、action 顺序、commit 来源、prepare 和 idempotency key 绑定到该层级。
- 已引入 compatibility object precondition、idempotency key 和 action checkpoint；后续继续绑定
  Pack operation 和 global stable object ID。
- 已为本地 `TaskPlanner` 实现 observe -> tool -> validate -> commit -> re-observe loop；远程路径
  已具备可撤销 project grant 和逐次 disclosure audit，仍待迁移为逐 action 多轮并加入跨轮预算。
- 本地 validation failure 和 object-version conflict 已结构化反馈并最多自动修复三次。
- 已实现保留后续对象写入的 compensating revert、双向审计关联和 conflict report；后续把
  authorization snapshot 扩展为可撤销 project policy，并绑定 Pack lock。

### Phase 4：Mechanical 深纵切

- 接入 Open CASCADE adapter 和 feature regeneration graph。
- 实现 stable topology reference、B-rep validation 和 content-addressed tessellation。
- 交付参数化 part、基础 assembly、drawing view、STEP boundary 和机械 Release policy。

### Phase 5：完整设计创作型 MCAD

- 扩展 surface、sheet metal、weldment 和 configuration family。
- 扩展 assembly/motion constraint、工程图和 MBD/PMI。

### Phase 6：平台能力

- 实现本地权威 commit 的云端异步复制和单写者 branch promotion。
- 完成 signed Release、企业审计、Pack 分发和大工程性能治理。
- 在核心与 Pack 契约稳定后，以 EDA Pack 验证跨领域扩展。

CAM、CAE solver 和 PLM/PDM 不属于当前设计创作型 MCAD 产品边界，只作为后续集成。

## 11. 质量门禁

当前 workspace 的基础门禁见[开发指南](development.md)。架构迁移必须优先增加以下
contract test：

- Planner 伪造 pass 或返回空 report 不能绕过本地 validator；
- hard error 不产生 action commit；
- 多个 Prompt 交错时不发生 silent overwrite；
- compensating revert 保留后续无关修改并报告冲突；
- action retry、pause、crash recovery 和 duplicate request 保持幂等；
- Pack/Core/geometry version lock 内 replay 得到一致 state hash；
- 旧 `.cadx` fixture 在迁移后仍通过完整 history replay。
