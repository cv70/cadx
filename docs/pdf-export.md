# PDF 2D 视图导出

> 文档类型：导出格式契约
> 状态：Implemented
> 适用范围：Current
> 权威内容：当前 PDF 页面、投影、损失报告和资源限制
> 返回：[文档索引](index.md)

CADX 从 immutable 2D render scene 导出有界的单页 vector drawing。这是当前视图的
presentation boundary，不是目标 Mechanical drawing/工程图子系统：它没有模型视图、
投影视图关联、图框或工程图更新语义。PDF 不保存可编辑 CAD entity、parameter、
constraint、semantic history、task、branch 或 recovery state。当前 `.cadx` 工程仍是
可编辑内容的权威表示。

## 页面设置

Workbench 支持以下页面：

- A4、A3 和 US Letter；
- portrait 或 landscape；
- 有限且非负的 margin。

CADX 将可见 scene 等比缩放并居中放入 printable rectangle，不修改 document 或
viewport camera。Geometry 保持为 vector content；line width、arrowhead 和 annotation
text 使用固定的 page-space 尺寸。

## 投影

| CADX render primitive | PDF projection |
| --- | --- |
| `Line` | Vector line segment |
| `Circle` | 四段 cubic Bezier curve |
| `Arc` | 每段不超过 90 度的 cubic Bezier curve |
| `AlignedDimension` | Extension line、dimension line、filled arrow 和格式化数值文本 |
| `Rectangle` | Closed vector path |
| `SketchProfile` | Open 或 closed vector path |
| `Wall` | 使用可见缩放厚度的 vector centerline |
| `Room` | Closed vector boundary |
| `Text` | ASCII 内容使用 Base-14 Helvetica |
| `Extrude` | 当前没有 3D drawing projection，计入 `skipped` |

Hidden layer 和 hidden entity 被省略并计入 `skipped`。Layer RGBA color 在白色
PDF 页面上 flatten。Parameter、constraint 和 layer-lock metadata 计入
`omitted`。

当前文本边界只支持 ASCII：

- 非 ASCII `Text` entity 被跳过；
- dimension 的非 ASCII override 仍保留 vector geometry，但省略 label 并计入
  `simplified`。

这属于显式损失报告，不会静默替换字符。嵌入 Unicode font 是后续扩展。

## 资源限制与原子性

编码前，CADX 验证完整 document 和页面选项，并对 immutable scene 执行以下预检：

| 资源 | 上限 |
| --- | ---: |
| 编码输出 | 64 MiB |
| Document entity | 250,000 |
| Vector path segment | 1,000,000 |
| Source text 总量 | 8 MiB |

完整 PDF 在内存中编码并验证后，才通过同目录 temporary file、file sync、atomic
rename 和 directory sync 替换目标文件。无效页面、无效 geometry、资源超限、编码失败
或写入失败时，已有目标文件保持不变。
