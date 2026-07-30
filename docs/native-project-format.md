# `.cadx` 原生工程格式

> 文档类型：文件格式契约
> 状态：Implemented
> 适用范围：Current
> 权威内容：当前 `.cadx` format v12、document schema、迁移和 recovery sidecar
> 返回：[文档索引](index.md)

当前 CADX 将可编辑本地工程保存为 `.cadx` ZIP archive。本文只定义已经实现的
format v12；目标工作存储向 SQLite/WAL 和内容寻址 blob 的演进见[目标架构](design.md)，
不得用目标设计重新解释现有 archive。

## Archive 布局

Format v12 必须且只能包含两个 regular file：

```text
manifest.json
workspace.json
```

`workspace.json` 是 `TaskWorkspace` 的无损 Serde 表示，包含：

- active document；
- 独立于 document/history 的 stable project UUID；
- project remote-access grant 当前状态与 append-only grant/revoke policy event ledger；
- semantic history 和 snapshot；
- caller validation claim 和 core-produced candidate validation evidence；
- 每个非 root commit 的 `PreparedActionRecord`；
- branch 与 branch-local redo stack；
- `DesignTask -> PromptChangeSet -> AgentRun` 层级，以及原始 Prompt、结构化目标、授权快照、
  运行身份、状态、诊断、事件和有序 action commit 引用；
- 每个 Task 的 append-only ChangeSet write-authorization revocation ledger；
- paused run 尚未执行的持久化 typed action，以及 plan 的 `base_revision`、当前
  `expected_revision`、action checkpoint、`next_action_preparation`、显式 execution strategy
  和不可放宽的 `planning_budget`；
- iterative run 的逐 action re-observation、规划完成事件、结构化失败反馈和自动修复次数；
- conflict-aware compensation 的目标 ChangeSet、请求 revision、恢复对象、结构化冲突、
  可选补偿 commit，以及 target/compensation 双向关联。

当前顶层 JSON 键为 `project_id`、`remote_access_policy`、`document`、`history`、`tasks`、
`next_task_id`、`next_change_set_id` 和 `next_agent_run_id`。Core 内部的私有
`DocumentStore` 只改变内存所有权，不会产生 `store` 包装键，也不会提升文件格式版本；reader
和 writer 使用显式 wire representation 保证该布局可反序列化和无损 round-trip。

Format v4 引入了实际网络调用前的 hash-bound remote-send audit：provider endpoint/model、
requested capability、selected object ID、source revision、固定数据类别、payload bytes 和
domain-separated SHA-256。它只记录披露和内容绑定，不保存完整 payload、响应 transcript 或
credential。

Format v5 为每个持久化 `TaskExecution` 增加 revision precondition。`base_revision` 绑定
Planner 观察并生成 action list 的 snapshot；`expected_revision` 初始等于 base，并在每个
action commit 成功后与 checkpoint 一起推进。

Format v6 为每个非 root `SemanticCommit` 和 pending task 的下一 action 增加
`PreparedActionRecord`：

| 字段 | 含义 |
| --- | --- |
| `base_revision` | 本地 prepare 所观察的历史 revision。 |
| `preconditions` | transaction 涉及的 layer/entity/parameter/constraint 的存在状态与最后修改 revision。 |
| `input_state_hash` | prepare base document 的 canonical candidate-state SHA-256。 |
| `candidate_state_hash` | transaction 在 prepare base 上模拟后的 document SHA-256。 |
| `idempotency_key` | 绑定 input/candidate hash、task 来源和 typed transaction 的 32-byte key。 |

对象版本从 active ancestry 派生；删除对象保留 tombstone revision，所以 delete/recreate ABA
不会满足旧 precondition。提交 prepared action 时，base 必须是当前 head 的祖先且 input hash
必须匹配该 base。无关对象的后续 commit 可以位于 base 与当前 head 之间；相同对象或依赖对象
发生变化时提交失败。相同 idempotency key 在当前 ancestry 上重试返回原 commit；原 commit
已因 undo/branch 切换离开当前 ancestry 时返回明确冲突，不生成重复几何。

Preparation 的 `candidate_state_hash` 绑定 prepare base 上的模拟结果；commit 的
`ValidationEvidence.candidate_state_hash` 绑定 transaction 实际应用到最终 parent 后的结果。
存在无关 interleaved commit 时两者可以不同，这不是完整 workspace CAS。对象 ID 当前仍是
document-local typed `u64`，不是跨项目或跨 replica 的 stable ID。

Format v7 将 task execution 迁入真实的 Prompt/Run 层级。每个 `SemanticCommit` 的来源从单一
`task_id` 扩展为 `task_id + change_set_id + agent_run_id`；run 内的 `action_commits` 以连续
action index 保存 commit 顺序。Run-bound preparation 使用 v2 idempotency domain，并把三层来源
共同纳入 key。远程上下文 schema v2 同时绑定 task、ChangeSet 和 Run；旧的 hash-bound schema
v1 audit 仍按其记录版本验证，不会被重写为 v2。

Format v7 不改变 document schema；format v6 到 v7 不改写模型 transaction 或 diff，但会为旧
task commit 补齐 ChangeSet/Run 来源并据此重新生成 preparation。

Format v8 增加 conflict-aware compensation audit，并给 `DocumentDiff` 增加
`deleted_parameters`，使目标 ChangeSet 创建的新参数可由前向 `DeleteParameter` command 精确
补偿。补偿不保存或信任 reverse patch；Core 从目标 commit parent snapshot 重建对象基线，在
`requested_at_revision` 核对最后修改 revision，并把未被后续写入的对象作为一个普通
run-bound ActionCommit 提交。冲突对象不被覆盖。若全部对象冲突，补偿 ChangeSet 仍持久化，但
不会生成空 commit。Format v7 到 v8 的缺失补偿字段迁移为 `None`，缺失 deleted-parameter diff
迁移为空；document schema 仍为 5。

Format v9 为每个 `TaskExecution` 增加显式 `strategy`。旧的 `batch` strategy 保留一次规划的
持久化 action list；本地 `iterative` strategy 在每个 action commit 后回到等待 Planner 的状态，
并持久化 `planner_complete` 与最近的 `ActionFailureFeedback`。反馈绑定 action index、观察
revision、失败类别、intent、tool、诊断和 `1..=3` repair attempt；对应 `Reobserved`、
`ActionRejected` 和 `PlanningCompleted` event 也进入 Run 审计。准备或提交失败的 action 不会进入
成功 checkpoint；同一 action 初始失败后最多自动生成三个修复 proposal，第四次失败终止 Run。
Format v0-v8 缺失 strategy 时只在显式旧格式迁移路径中解释为 `batch`；当前格式缺失或篡改
strategy、feedback、completion/re-observation event 会被拒绝。该升级不改变 document schema。

Format v10 增加 stable project UUID 和持久化 `RemoteAccessPolicy`。每个 grant 绑定 project、
endpoint/model、允许的数据类别与 capability、project summary 或 selected entity ID 范围、
payload 上限、创建时间、可选到期时间和撤销时间。`Granted` event 保存完整原始 grant，
`Revoked` event 只追加 grant ID 与撤销时间；loader 重放整个 ledger 并要求其与当前 grant map、
连续 ID cursor 和 project ID 一致。

远程上下文 schema v3 在 payload 中加入 project ID。每个新 `ProviderDisclosure` run event 同时
绑定 project/grant ID 与发送时间，并继续保存 task/ChangeSet/Run 所在位置、source revision、
endpoint/model、capability、selection、数据类别、payload bytes 和 SHA-256。完整 payload、
Provider 响应和 credential 仍不进入 archive。Format v10 archive 缺少 project ID 或
policy、grant ledger 无法重放、发送事件引用不存在或不覆盖该披露的 grant 时直接拒绝。
用户级 `egress-policy.yaml` 也不进入 archive；加载或复制工程只会恢复项目 grant，不能恢复、
扩大或绕过当前机器的 endpoint/model 出口策略。每次新发送仍必须重新通过本机策略。

Format v11 为每个 `TaskExecution` 增加持久化 `TaskPlanningBudget`。Batch execution 的
`max_actions` 必须精确等于 action list 长度，`max_decisions` 必须为 1。Iterative execution 的
`max_actions` 必须位于 `1..=256`，decision 上限固定为：

```text
max_decisions = max_actions * (MAX_AUTOMATIC_REPAIR_ATTEMPTS + 1) + 1
```

当前最大自动修复次数为 3，因此该上限同时覆盖每个 action 的初始 proposal、最多三个修复
proposal，以及最终 complete decision。每次 `Reobserved` 消耗一个 decision 名额；暂停、保存、
崩溃恢复或调用方传入更大的运行 budget 都不能扩大持久总上限。

当前远程上下文 schema v4 增加 `ExecutionState` 数据类别，并在 payload 中绑定 action index、
总/剩余 action 与 decision budget，以及可选的最近失败反馈。Remote iterative Run 的每个
Provider decision 必须按 `Reobserved -> schema-v4 ProviderDisclosure -> action|complete` 顺序
绑定同一 source revision；每个 observation 只能被一次发送审计和一个 decision 消费。缺失、
重复、错序或绑定其他 revision 的审计均使当前 workspace 无效。完整 payload、Provider 响应和
credential 仍不进入 archive。

Format v12 为每个 `DesignTask` 增加 `authorization_revocations`。每条撤销记录绑定
`change_set_id`、`revoked_at_revision`、撤销前已经提交的 `committed_action_count` 和非空原因。
原 `PromptChangeSet.authorization` 保持不变，用于验证历史 commit 当时是否处于授权范围；当前
有效性由 revocation ledger 派生。每个 ChangeSet 最多一条撤销，目标 ChangeSet 必须存在且原本
具有 direct-write authority，revision 必须存在，撤销后该 ChangeSet 的 action commit 数不得再
增加。撤销发生在已审计的远程调用之后时，Provider 输出仍必须在 stage/commit gate 被拒绝。

它不得包含 credential 或 provider secret。

`manifest.json` 字段如下：

| 字段 | 含义 |
| --- | --- |
| `format_version` | Native archive format version |
| `document_schema_version` | Active document 的 schema version |
| `workspace_entry` | 必须为 `workspace.json` |
| `workspace_bytes` | 未压缩 workspace payload 的精确字节数 |
| `workspace_crc32` | 未压缩 workspace payload 的 CRC-32 |

使用 map 保存的模型数据按确定 key order 序列化。工程内容不变时，除 ZIP metadata
外，输出保持稳定。

## Document Schema

当前 reader 支持以下历史 schema 并迁移到 schema 5：

| Schema | 引入的能力 | 迁移规则 |
| --- | --- | --- |
| 1 | 初始 document | 增加空 constraint graph，规范化 ID cursor，缺失 formula 按 literal 处理。 |
| 2 | Parameter formula 和持久化 2D constraint graph | Formula source 保留为可编辑文本并在本地重新解析；solver output 作为普通 entity-update command 写入 history。 |
| 3 | Layer locking 和 layer update/delete diff | 缺失的 lock 迁移为 `false`。 |
| 4 | 精确 drafting `Arc` | 保存有限 center、正 radius、radian start angle 和逆时针 partial sweep；display tessellation 不序列化。 |
| 5 | 精确 `AlignedDimension` | 保存两个 measured point、非零 signed offset 和可选 DXF `<>` text template；arrow 与 label layout 由 renderer 派生。 |

Schema 3 到 4、schema 4 到 5 不重写已有 geometry，但 active document 和所有历史
snapshot 都必须升级后重新执行 history replay 检查。

## Recovery Sidecar

工程 `part.cadx` 的 crash recovery 路径为：

```text
.part.cadx.autosave.cadx
```

Sidecar 是完整的当前格式 `.cadx` archive，不是 primary archive 的额外 member，
也不使用另一套 JSON schema。因此它必须经过与普通保存相同的 payload limit、CRC、
migration、history replay、task validation、temporary write、file sync、rename 和
directory sync。

应用通过 `symlink_metadata` 检查自动发现的 sidecar，只接受非符号链接的 regular
file。Workspace 发生变化并空闲两秒后，应用 clone 当前 workspace，在专用 writer
thread 中编码。正常退出时先等待进行中的 writer，再同步保存最新状态，防止旧 snapshot
在 rename race 中覆盖新状态。

Sidecar 不会与 primary 静默合并：

- startup 或显式 Open 发现 sidecar 后，必须由用户选择 `Recover` 或 `Discard`；
- `Recover` 完整验证 archive 后才安装 workspace，并保持 dirty，直到 primary save 成功；
- `Discard` 删除 sidecar 并同步目录；
- primary save 成功后执行相同清理；
- primary save 或 cleanup 失败时保留 recovery state。

## Load 契约

Loader 从不将 archive member 解压到文件系统。它执行以下检查：

1. 只允许 `manifest.json` 和 `workspace.json`，拒绝 duplicate 或 extra entry。
2. JSON deserialize 前限制 manifest 为 64 KiB、workspace 为 64 MiB。
3. Format v1-v12 的 payload length 与 CRC-32 必须匹配 manifest。
4. 迁移支持的 document schema。
5. 重放所有 commit，核对每个 recorded diff、snapshot 和 candidate-state validation evidence。
6. 对每个 replayed candidate 重新执行当前 core validator；evidence 缺失、版本不匹配、
   hash/check 不一致或 hard failure 都拒绝 archive。
7. 对每个非 root commit 从 recorded preparation base 重建 preparation，核对 ancestor、
   transaction/source、对象 precondition、input/candidate hash 和 idempotency key。
8. 验证 branch reference、连续的 branch-local redo stack，以及每个 task commit 的
   task/ChangeSet/Run 三层 ownership。
9. 验证 task、ChangeSet 和 Run ID 全局不重复且 map/parent/active binding 一致；历史 ChangeSet
   和 Run 必须处于终态，task/active ChangeSet/active Run 状态必须一致，失败或取消结果必须保留
   非空诊断。
10. 验证每个 run execution 的 base/expected revision 存在，action commit 数与 checkpoint
   一致；run output 从 base 开始保持 ancestor 顺序、action index 连续并归属于该 run，允许其间
   穿插无关 commit；expected revision 等于最后一个 run output（无 output 时等于 base）。Pending
   action preparation 的真实 base 必须位于 expected revision 与 active head 之间，并与 typed
   transaction 和三层来源完全匹配。
11. 验证 execution strategy 与审计事件：当前格式必须显式声明 batch/iterative；iterative
   completion、repair feedback、re-observation revision/action index 必须与 checkpoint 一致，
   历史 revision 的实体计数必须可重算，修复次数不得超过三次；planning budget 必须匹配
   strategy，累计 action 和 decision 不得超过持久上限。
12. 验证 task write-revocation ledger：当前格式必须显式保存台账；ChangeSet 和 revision 必须
   存在，原因非空，记录不得重复，`committed_action_count` 必须等于该 ChangeSet 的最终输出数。
13. 验证 compensation audit：target/compensation 双向关联、request revision、active ancestry、
   对象基线与最后修改 revision、恢复/冲突全集、补偿 commit parent/source 和最终恢复状态必须
   一致；伪造 `Reverted`、冲突 revision、对象列表或 commit link 都拒绝。
14. 验证 project identity 和 remote policy：grant 必须绑定当前 project，重放 append-only
   grant/revoke ledger 必须精确恢复当前 grant map 与 ID cursor。
15. 验证 remote-send audit：hash-bound 记录必须引用存在的 source revision，使用支持的 context
   schema，包含非空数据类别、`1..=64 KiB` payload length 和 64 位十六进制 SHA-256；带 grant
   binding 的记录必须同时包含 project ID、grant ID 与发送时间，且该 grant 当时有效并覆盖
   endpoint/model、数据类别、capability、selection 和 payload bytes。Remote iterative Run 还
   必须为每次 observation 提供一个绑定相同 revision、包含 `ExecutionState` 的 schema-v4 审计。
16. 验证 active document 等于 active branch head 的 replay 结果。

中断时处于 running 的持久化 task：

- 有 `TaskExecution` 时恢复为 paused checkpoint；这包括 iterative Run 等待下一次 Planner
  decision 的边界；
- 尚未建立 durable execution 时标记 failed，不能含糊地自动恢复。

任一步失败都拒绝打开工程。

Format v0 没有 manifest checksum，只能作为 migration source 读取。Format v1
早于持久化 branch-local redo state，缺失的 redo map 初始化为空。Format v2 早于本地
validation evidence；loader 只在读取 v0-v2 时，按完整 replayed candidate 重新生成 evidence。
Format v3 archive 缺失 evidence 时直接拒绝，不走 legacy regeneration。所有旧格式都必须
重新保存为 v12；高于当前 reader 的 format version 会被拒绝。

Format v3 早于 hash-bound remote-send audit。旧 `ProviderDisclosure` 缺失的
`context_schema_version`、`source_revision`、`data_categories`、`payload_bytes` 和
`payload_hash` 迁移为空的 legacy audit，保留原 provider、capability、selection 和摘要，
不会伪造无法从旧记录恢复的 payload hash。运行日志必须将其标识为旧版未绑定记录。Legacy
audit 可以随迁移后的 workspace 保存，但不得被解释为 hash-bound 发送内容证明；任何部分
填写或格式错误的当前审计记录都会被拒绝。

Format v4 早于 revision-bound task execution。Loader 只对 v0-v4 使用显式 legacy migration：
若已有 output commit，以第一个 output 的 parent 作为 base，以 checkpoint 对应的最后一个
output 作为 expected；没有 output 时以读取时的 active head 作为两者。推导后仍必须通过上述
连续 chain 和 ownership 检查。Format v5 缺失任一 revision 字段时直接拒绝，不使用 legacy
推导；篡改 expected revision、checkpoint 或非连续 output chain 也会拒绝。

Format v5 早于 action preparation。Loader 只对 v0-v5 从每个 commit 的 parent snapshot
推导 commit preparation，并从 task checkpoint snapshot 推导 pending action preparation；
推导过程重新执行 typed transaction 与本地 validator。Format v6 缺失 preparation、篡改
对象 precondition、input/candidate hash、idempotency key、base 或 transaction/source 绑定时
直接拒绝，不使用 legacy 推导。Root commit 必须且只能没有 preparation。

Format v6 早于 PromptChangeSet/AgentRun 层级。Loader 只对 v0-v6 将旧 task 的 goal、authority、
status、event、output commit 和 execution 无损归入一个初始 ChangeSet 与 Run；旧 output commit
按 checkpoint 顺序成为 run action commit，并写入完整三层来源。迁移会使用新来源重新生成这些
commit 及 pending action 的 preparation。Format v7 出现 legacy 顶层 `events`、
`output_commits` 或 `execution`，缺失 active ChangeSet/Run，ID 重复/错绑，action index/commit
ownership 错误，或 commit source/preparation 不一致时直接拒绝，不使用 legacy migration。

Format v7 早于补偿字段与参数删除 diff。Loader 只对 v0-v7 将缺失的 `compensation`、
`reverted_by` 初始化为空，并把缺失的 `deleted_parameters` diff 初始化为空；迁移后仍执行完整
workspace replay。Format v8 的补偿记录必须通过上述关联和状态重建校验。

Format v8 早于显式 execution strategy。Loader 只对 v0-v8 将缺失 strategy 的已有 plan 标记为
`batch`；format v9 缺失 strategy 直接拒绝。迁移不会把旧批计划伪装成 iterative，也不会生成旧
archive 中不存在的 re-observation、repair 或 completion event。

Format v9 早于 stable project identity 和 remote policy。Loader 只对 v0-v9 生成新的 project
UUID 和空 grant map/event ledger；旧 remote audit 保留原 schema 与 binding 状态，不会被改写为
由新 grant 授权。Format v10 缺失 `project_id` 或 `remote_access_policy`、grant 状态与 ledger
不一致、ID cursor 非连续，或当前 schema v3 audit 的 project/grant binding 无效时直接拒绝。

Format v10 早于显式 planning budget。Loader 只对 v0-v10 填充缺失 budget：batch 使用持久
action list 的精确长度和一次 decision；iterative 使用旧 core 的 256 action 上限及由固定公式
推导的 decision 上限。迁移不会依据调用方本次运行参数猜测更小上限，也不会把旧 batch Run
改写为 iterative。Format v11 缺失 budget、action 上限越界、decision 公式不匹配、累计使用量
超限，或 schema-v4 remote iterative audit 缺少 `ExecutionState`/逐轮绑定时直接拒绝。

Format v11 早于 task write-authorization revocation ledger。Loader 只对 v0-v11 为每个 Task
添加空台账，不会推断或伪造旧 archive 中不存在的撤销；旧 format 标记中夹带非空 v12 台账会
被拒绝。Format v12 缺失台账、记录重复、引用
不存在的 ChangeSet/revision、原因为空、撤销 ReviewOnly ChangeSet，或撤销记录的 action 数与
最终 ChangeSet output 不一致时直接拒绝。

## Save 契约

保存过程固定为：

1. 验证完整 workspace。
2. 在内存中编码完整 archive。
3. 在目标目录创建唯一 temporary file。
4. 写入并同步 temporary file。
5. Rename 为目标工程路径。
6. Unix 上同步 containing directory。

Write 或 rename 失败时删除 temporary file。对于支持同目录原子替换的文件系统，
每次成功保存形成 all-or-nothing replacement boundary。这不能替代普通 backup；硬件故障、
恶意修改和长期 retention 仍需文件系统或托管备份。

## 兼容性规则

- Reader 可以在内存中向前迁移支持的旧格式。
- Reader 不得静默解释未来格式。
- 新 document schema 必须提供显式 migration 和 fixture。
- 新 archive member 必须提升 format version 并修改 loader allowlist。
- Recovery sidecar 必须保持为完整 native archive，不得绕过正常验证或原子保存。
- Credential、包含 secret 的 provider transcript 和 executable content 不得进入 archive。
- CRC-32 只用于损坏检测，不构成防篡改审计；目标 hash chain 和 signed Release
  见[目标架构](design.md)。
- Candidate state SHA-256 绑定 evidence 与 replayed document，但没有 signer 或密钥，
  不构成来源认证或 Release 签名；精确边界见[本地验证证据](validation-evidence.md)。
