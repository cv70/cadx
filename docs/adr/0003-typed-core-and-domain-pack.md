# ADR-0003：强类型核心与 Domain Pack

> 文档类型：架构决策记录
> 状态：Accepted
> 适用范围：Target
> 权威内容：为何采用强类型通用核心和原生动态 Domain Pack
> 返回：[文档索引](../index.md) · [ADR 索引](README.md)

## 背景

通用 CAD 核心需要跨领域保存身份、依赖、参数、约束和历史，同时让机械、EDA 等领域扩展自己的语义。自由字符串类型和无约束 JSON 容易绕过量纲、引用和迁移检查；直接暴露 Rust/C++ ABI 又无法提供稳定插件兼容性。

机械纵切需要成熟的 B-rep 和拓扑算法，但几何内核的对象布局不应成为工程格式或 Agent 接口。

## 决定

Core 采用强类型通用模型，负责全局稳定 ID、对象版本、带角色关系、依赖、约束、来源和带量纲 `Quantity`。关键几何值采用规范化整数数据库单位。安全关键领域数据放入带 `schema_id` 和版本的 typed protobuf payload，不使用无约束 `Map<String, Value>`。

`DomainPack` 采用原生动态库，由 manifest 描述能力和版本。宿主与 Pack 通过稳定 C ABI vtable 调用，通过 protobuf 交换消息，不承诺 Rust ABI、C++ 类布局或几何内核对象可跨边界传递。工程的 `PackLock` 固定所有影响重放、再生与验证的依赖；完整锁定集合由[目标设计](../design.md)定义。

Mechanical Pack 使用 OCCT 作为首个 B-rep 实现。参数化 Feature Graph 是权威状态，B-rep 和网格是内容寻址派生缓存。持久引用使用生产特征、语义角色和几何签名；再生结果必须明确为 resolved、ambiguous 或 missing，禁止按瞬时面/边索引静默重绑。

## 接口影响

- `DocumentStore` 私有，Pack 只能通过只读 snapshot 和 `KernelFacade` 交互。
- Pack manifest 必须声明 ABI、schema、操作、查询、validator、迁移器、artifact hash 和信任元数据。
- Pack 返回候选对象变化、派生物和诊断；Core 检查身份、引用、量纲、前置条件和验证证据后才提交。
- 缺少锁定 Pack 或显式迁移器时，工程只能以只读/诊断模式打开。

## 替代方案

- **字符串类型加任意属性 map**：拒绝。无法可靠验证、迁移或生成工具 schema。
- **把所有领域类型编译进 Core**：拒绝。会让通用内核随每个领域变化。
- **直接使用 Rust trait ABI 或 C++ 对象 ABI**：拒绝。编译器和依赖版本会破坏二进制兼容。
- **强制所有 Pack 进入 WASM/子进程沙箱**：暂不采用。首阶段优先低延迟 OCCT 集成；不受信扩展隔离应另立契约。

## 后果

核心不变量可统一验证，工具和迁移可由 schema 驱动，几何实现也能在不改工程语义的前提下演进。代价是 Pack 作者必须维护 manifest、protobuf schema、迁移器和 C ABI 适配层；跨边界调用也需要显式错误与资源所有权协议。

## 安全限制

原生 Pack 在宿主进程内拥有与 CADX 相同的权限。artifact hash、签名和版本锁只能标识与复现代码，不能提供运行时隔离。加载未知 Pack 后，工程验证、凭据和本机数据的安全保证必须按不受信本地代码处理。
