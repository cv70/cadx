# 本地验证证据

> 文档类型：组件实现契约
> 状态：Partial
> 适用范围：Current
> 权威内容：当前 candidate commit gate、state binding、迁移和信任限制
> 返回：[文档索引](index.md)

CADX 把调用方描述和提交证据分为两个不同信任来源：

- `TaskAction.validation` / `SemanticCommit.validation` 是 caller 或 Planner claim；内容可以
  为空、warning 或 failed，只作为审计输入，不决定 commit 是否通过；
- `ValidationEvidence` 只能由 `cadx-core` 在 transaction 已应用到 candidate clone 后生成，
  是当前 semantic commit 的本地 admission gate。

Planner 不能构造 `ValidationEvidence`，也不能通过返回空 report 或伪造 passed check 绕过
本地检查。Hard failure 返回 `HistoryError::CandidateValidationFailed`，candidate、history
head、ID cursor 和 active document 都保持不变。

## Prepare 与 Admission 顺序

每个 human 或 Agent transaction 使用同一内核路径：

1. `PreparedAction::prepare` 验证 revision-bound input snapshot，在 clone 上应用完整
   transaction，并对 prepare candidate 运行 `cadx.core.candidate@1`。
2. Prepare 记录 base revision、transaction 涉及对象的版本 precondition、input/candidate
   state hash 和绑定 task/source 的 idempotency key；短期 `PreparedAction` 不能反序列化。
3. Commit 时重新确认 base 是当前 head 的祖先、base input hash 未变化，并逐个比较对象
   precondition。无关对象的 interleaved commit 不阻止提交，同对象、依赖对象和 ABA 变化会拒绝。
4. Transaction 在当前 document clone 上再次原子应用，得到最终 candidate 和 diff；Core 对
   最终 candidate 重新运行 validator。
5. 只有最终 evidence 没有 failed check 时才分配 commit ID、移动 branch head 并安装 candidate。
6. Task commit 追加 `Validated` audit event，记录 validator、version、最终 state hash 和摘要。

`PreparedActionRecord.candidate_state_hash` 与 `ValidationEvidence.candidate_state_hash` 有不同
时间点：前者绑定 prepare base 上的模拟 candidate，后者绑定最终 parent 上实际提交的
candidate。没有 interleaved edit 时二者相同；存在无关 edit 时可以不同。Commit 不把旧
prepare hash 当成最终 document hash，而是重新验证最终 candidate。

相同 idempotency key 在当前 ancestry 上重试返回已有 commit；如果该 commit 已被 undo 到
非当前 ancestry，则返回 `IdempotencyConflict`。重试不会分配新的 commit ID。

补偿回滚不绕过该顺序。Core 从历史 snapshot 生成新的 typed transaction，对请求 revision
prepare，并使用目标 ChangeSet 的 authorization snapshot通过同一 admission gate。冲突对象不会
进入 transaction；无冲突对象的补偿 commit 具有独立 evidence 和 task/ChangeSet/Run 来源。

当前 evidence 包含：

| 字段 | 当前语义 |
| --- | --- |
| `validator_id` | 固定为 `cadx.core.candidate`。 |
| `validator_version` | 当前为 `1`；变更 check 或 hash contract 必须提升版本。 |
| `candidate_state_hash` | 32-byte SHA-256，绑定完整 candidate document。 |
| `report.checks` | Core 本地产生的 ordered structured checks。 |

State hash 的 domain separator 是 `CADX-CANDIDATE-STATE\0canonical-json-v1\0`，后接
`CadDocument` 的 deterministic Serde JSON。Document map 使用 `BTreeMap` stable key order；
hash writer 限制 64 MiB，不为 hash 额外保留完整 payload。Hash contract 只覆盖 document，
不覆盖 task event、caller claim、branch metadata 或未来 `PackLock`。

## 当前 Checks

`Core document structure` 检查 schema、layer/entity/parameter/constraint identity、finite
geometry、reference、parameter expression、layer lock 和 ID cursor 等现有 document invariant。

`Sketch constraint system` 使用默认的 deterministic solver：

- driving constraint 不收敛：failed，阻止 commit；
- 收敛但 candidate 仍有未应用的 solver entity update：warning，允许 Draft commit；
- 已满足或没有 constraint：passed。

当前 solver 只是有限的 2D projection solver。Evidence 不证明 B-rep 有效、profile 可制造、
装配可解、STEP 等价、材料规则通过或完整 MCAD 约束收敛。

## Persistence 与 Replay

`.cadx` format v3+ 为每个 root/action commit 保存 evidence。Save 前和 load 后都 replay
history，并针对每个 replayed candidate 重新计算当前 evidence；缺失、validator version
不匹配、hash/check 不一致或 local hard failure 都拒绝 workspace。

Format v0-v2 早于该字段，loader 在显式 legacy migration 路径中从 replayed candidate
重新生成 evidence。Format v3+ 缺失 evidence 时不会降级到 regeneration。Format v4
增加 hash-bound remote-send audit，format v5 增加 task execution revision precondition，
format v6 增加 commit/pending-action preparation。Format v7 增加
`DesignTask -> PromptChangeSet -> AgentRun` 层级，把 commit source 与 idempotency key 绑定到
task/ChangeSet/Run，并将远程上下文 schema 提升为 v2。Format v8 增加补偿关联、冲突记录
和参数删除 diff；Format v9 增加显式 batch/iterative execution strategy、逐 action
re-observation 与结构化 repair feedback；Format v10 增加 stable project identity、remote
grant ledger 和 schema-v3 audit binding；Format v11 增加持久 planning budget，并要求当前
remote iterative decision 具有 schema-v4 execution-state audit；Format v12 增加 ChangeSet
write-authorization revocation ledger。Loader 重新构造 preparation，并拒绝
缺失或篡改的层级 ownership、对象 precondition、hash 和 idempotency key；v0-v6 只通过显式
legacy migration 进入新层级。这些升级都不改变 document schema。格式细节见
[原生工程格式](native-project-format.md)。

## 信任限制

SHA-256 提供 candidate 内容绑定和篡改检测基础，但当前 evidence 没有 Pack lock、commit
hash chain、设备身份或数字签名。任何能修改 archive 并重新计算 CRC 的主体都能修改普通
工程内容；loader 的保证是用本地代码重新验证结果，不是证明文件来自谁。

当前 `KernelFacade` 已成为 workspace 唯一公开写入口，并把 transaction 交给私有
`DocumentStore` 的本地 commit gate；本地 `PreparedAction`、对象版本 precondition 和
幂等重试已经实现。尚未实现的是 Pack lock、Pack validator、global stable object ID、B-rep
evidence、Release policy、commit hash chain 和签名 attestation；这些仍按
[ADR-0005](adr/0005-validation-and-release.md) 与[目标架构](design.md)演进。
