# 机械视口

> 文档类型：组件实现契约
> 状态：Partial
> 适用范围：Current
> 权威内容：当前 3D extrusion scene、交互行为和实现限制
> 返回：[文档索引](index.md)

CADX 为现有 `SketchProfile` 和 `Extrude` entity 提供第一版可用 3D 视图。
Document 和 semantic history 仍是权威状态；triangle mesh、camera projection、
shading 和 pick data 都是 immutable derived render state，不会序列化进 `.cadx`。

## 支持的 Geometry

每个可见 `Extrude` 解析其引用的 closed `SketchProfile`，并从 `z = 0` 沿 `+Z` 生成包含
bottom、top 和 side face 的 closed triangle mesh。被引用 profile 可以隐藏，planar outer
boundary 可以顺时针、逆时针或凹多边形。Triangulation 前会移除重复 closing point 和
连续重合点。

| Document entity | 3D 行为 |
| --- | --- |
| 可见 `Extrude` | 生成并显示 solid mesh。 |
| Closed polygonal `SketchProfile` | 只作为 `Extrude` 数据源，不单独显示。 |
| `Circle`、`Line`、`Arc`、`Rectangle`、`Wall`、`Room`、`Text` | 不进入 mechanical scene。 |

独立 `Circle` 即使命名为 mounting hole，也不会从 solid 中减料。当前没有 hole 或 boolean
语义，不能把这类 2D entity 解释成真实孔。

当前只支持一个没有 hole 的简单 planar outer profile。以下情况会被拒绝：

- 非有限坐标；
- 零面积 profile；
- 非正 extrusion distance；
- 不完整 triangulation；
- 缺失或无效的 profile reference。

Hidden extrusion 或位于 hidden layer 的 extrusion 不进入 scene。Locked extrusion
保持可见，但不能被 pick。

这些数值是拒绝异常输入的 safety limit，不是交互性能承诺：

| 资源 | Safety limit |
| --- | ---: |
| 单个 source profile 的 vertex | 250,000 |
| 单个 scene 的 derived vertex | 1,000,000 |
| 单个 scene 的 derived triangle | 2,000,000 |

## 交互与渲染

Workbench 默认进入 2D drafting view。在 `3D` 模式中：

- primary-button drag 旋转 perspective camera；
- mouse wheel 缩放；
- `Fit` 框选全部可见 solid；
- click 选择最近的 unlocked projected solid。

当前没有 3D pan、orthographic mode、view cube 或直接几何编辑。

Extrusion triangulation 和 immutable `MechanicalScene` extraction 在 CPU 上完成。App 将
scene 转换为 `f32` position、RGBA8 layer color、`u32` triangle/feature-edge index 和
entity ID，然后通过 `eframe` 的 `wgpu` paint callback 提交。当前 GPU backend 使用
`Depth32Float` depth buffer、4x MSAA、独立 face/edge pipeline、shader directional
lighting 和 selected-entity highlight。Face pipeline 使用小幅 depth bias，随后绘制的
feature edge 不写 depth；这避免共面 edge 与 face 发生 z-fighting，同时仍由 depth test
遮挡背面的 edge。

Solid picking 仍在 CPU 上对 projected triangle 做最近命中测试，不使用 GPU readback。
只要一个可见 solid 无效，renderer 就报告 extraction error，而不是显示不完整 geometry。
GPU scene 转换或单帧 camera validation 失败时，viewport 显示错误，不回退到 CPU solid
renderer。

Mechanical scene 按 semantic history head 缓存。Model 或 layer commit 后按需重建，GPU
buffer 仅在 scene revision 变化时重新上传；安装其他 workspace 时缓存失效并执行初始
fit。Camera movement 只更新 uniform，不会重新 triangulate，也不会写入 semantic history。

Scene extraction、triangulation 和 GPU scene conversion 在 UI 路径同步执行，较大 scene
首次显示或 commit 后可能阻塞一帧。每次 click picking 都会投影并分配全部可见 triangle，
时间与 triangle 数量线性相关；当前没有 spatial acceleration 或 GPU readback picking。

## 验证范围

| 平台 | 当前验证 |
| --- | --- |
| macOS / Apple M3 / Metal | 2026-07-29 完成 native startup、4x MSAA、depth/shading、fit、orbit、CPU picking、selection highlight 和 resize manual pixel smoke。 |
| Windows / Linux | CI 配置覆盖 build 和纯逻辑 unit test；尚无真实 WGPU adapter/surface 或 pixel smoke 记录。 |

现有 GPU unit test 覆盖 scene materialization、完整 entity ID、数值边界、camera matrix 和
CPU/GPU projection 一致性，但不会创建设备、pipeline 或 surface，也没有 canvas pixel
assertion。平台 smoke 不能替代 device-loss 和多 adapter 测试。

## 当前边界

这是一套 tessellated extrusion viewer，不是 B-rep kernel。当前不支持：

- profile hole 或 curved profile；
- boolean、fillet 和 chamfer；
- assembly 和 feature regeneration graph；
- tessellation LOD、section 和 manufacturing topology；
- STEP exchange 和 drawing-view generation；
- off-screen GPU picking、device-loss recovery 和 CPU rendering fallback；
- GPU `f32` 大坐标精度治理和异步 mesh preparation。

整个 native app 当前强制使用 `wgpu`，adapter/surface 初始化失败时没有 Glow 或 CPU
startup fallback；CPU projection 只服务 camera、fit 与 picking。目标 Mechanical Pack、
Open CASCADE 边界和稳定拓扑引用见[目标架构](design.md)。
