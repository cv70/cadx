# DXF 交换契约

> 文档类型：交换格式契约
> 状态：Implemented
> 适用范围：Current
> 权威内容：当前 DXF 导入导出的支持矩阵、损失语义和资源限制
> 返回：[文档索引](index.md)

DXF 在 CADX 中是有界、有损的交换格式。只有当前 `.cadx` 原生格式能保留 semantic
history、task、branch、parameter、constraint、feature relationship、layer lock
和 recovery state。

## 导入

导入只接受普通、非符号链接的 DXF 文件。解析器将支持的实体转换为 typed CADX
command，在文档 clone 上预检完整 transaction，最后由 workbench 生成一个 semantic
commit。解析、资源上限、单位转换、locked layer、ID 或 geometry 任一检查失败时，
workspace 保持不变。

当前支持 model space 中的以下子集：

| DXF entity | CADX entity | 条件 |
| --- | --- | --- |
| `LINE` | `Line` | XY 坐标有限，Z 为零，extrusion direction 指向正 Z。 |
| `CIRCLE` | `Circle` | radius 为正，center Z 为零，normal 指向正 Z。 |
| `ARC` | `Arc` | radius 为正，center Z 为零，normal 指向正 Z，逆时针 sweep 非零。 |
| aligned `DIMENSION` | `AlignedDimension` | definition point 不重合且有限，normal 指向正 Z，dimension line offset 非零。 |
| `LWPOLYLINE` | `SketchProfile` | open 至少两个 vertex，closed 至少三个；elevation 和 bulge 为零。 |
| 2D `POLYLINE` | `SketchProfile` | 不含 3D、mesh、spline、curve-fit、bulge 或非零 elevation。 |
| `TEXT` | `Text` | 文本非空，Z 为零，normal 指向正 Z。 |

Paper space、非平面内容、曲线 polyline 和不支持的 entity 会计入 `skipped`。
CADX 不会静默地把 DXF bulge 或 3D entity 拉直为 2D geometry。

`$INSUNITS` 会精确转换为打开文档的 millimeter、meter 或 inch。没有单位的 drawing
按当前 document unit 解释，并在结果中报告。DXF layer 与现有 CADX layer
按名称大小写不敏感匹配：

- 匹配且 unlocked 的 layer 被复用；
- 匹配 locked layer 会拒绝整个导入；
- 新 layer 保留 visibility 和 indexed color；
- 非法名称或名称冲突会被确定性重命名。

## 导出

导出读取 immutable document projection，在内存中编码 R2018 DXF，然后执行同目录
temporary file、file sync、atomic rename 和 directory sync。导出不修改 semantic
history 或 project dirty state。

| CADX 内容 | 导出结果 |
| --- | --- |
| `Line`、`Circle`、`Arc`、`AlignedDimension`、`Text` | 保留对应 DXF entity type。 |
| `AlignedDimension` | 保留 measured point、signed line offset 和可选 `<>` text template。 |
| `Rectangle`、`SketchProfile`、room boundary | 简化为 `LWPOLYLINE`。 |
| `Wall` | 简化为 centerline `LINE`。 |
| `Extrude` | 计入 `skipped`。 |
| parameter、constraint、layer lock | 计入 `omitted metadata`。 |

Rotated、radial、angular、diametric 和 ordinate dimension 尚不支持，计入
`skipped`。Layer visibility、entity visibility、document unit 和最接近的 indexed
layer color 会被写出。

## 资源限制

导入在 workspace commit 前、导出在替换目标文件前执行以下检查：

| 资源 | 上限 |
| --- | ---: |
| 编码后的输入或输出 | 64 MiB |
| Entity | 250,000 |
| Layer | 4,096 |
| Polyline vertex 总数 | 1,000,000 |

Workbench 中的 DXF path 与 native project path 相互独立，exchange operation
不得改变 `.cadx` save 或 recovery 的目标路径。
