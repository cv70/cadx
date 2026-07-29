# Third-Party Notices

CADX currently uses the following third-party crates:

| Dependency | License | Purpose |
| --- | --- | --- |
| `serde` | MIT OR Apache-2.0 | Versioned document and history serialization contracts. |
| `serde_json` | MIT OR Apache-2.0 | Native project archive payload serialization. |
| `serde_yaml` | MIT OR Apache-2.0 | Private user-scoped provider configuration and interface-preference parsing. |
| `crc32fast` | MIT OR Apache-2.0 | Native project payload integrity checks. |
| `zip` | MIT | Native `.cadx` archive container. |
| `dxf` | MIT | Bounded DXF/DXB parser and DXF writer used by the 2D exchange adapter. |
| `pdf-writer` | MIT OR Apache-2.0 | Low-level vector PDF encoder used by the bounded drawing export adapter. |
| `lopdf` | MIT | Test-only parser used to validate generated PDF structure. |
| `earcutr` | ISC | Bounded triangulation of planar sketch profiles for derived 3D extrude meshes. |
| `bytemuck` | Zlib OR Apache-2.0 OR MIT | Checked plain-data casts for bounded GPU vertex and uniform uploads. |
| `wgpu` | MIT OR Apache-2.0 | Cross-platform GPU rendering backend for the depth-tested mechanical viewport. |
| `sha2` | MIT OR Apache-2.0 | SHA-256 candidate-state binding for locally generated validation evidence. |
| `url` | MIT OR Apache-2.0 | Remote-provider endpoint validation. |
| `genai` | MIT OR Apache-2.0 | OpenAI Responses-compatible provider client. |
| `tokio` | MIT | Synchronous task-agent runtime bridge for remote requests. |
| `eframe` / `egui` | MIT OR Apache-2.0 | Native desktop workbench shell. |
| `sys-locale` | MIT OR Apache-2.0 | System-locale detection for the initial interface language. |
| `tempfile` | MIT OR Apache-2.0 | Private temporary files and cross-platform atomic interface-preference replacement. |
| Droid Sans Fallback | Apache-2.0 | Bundled CJK glyph fallback for the English and Simplified Chinese desktop interface. |

The Droid Sans Fallback copyright notice and complete Apache License 2.0 text are
distributed with the font at
[`crates/cadx-app/assets/DroidSansFallback-LICENSE.txt`](crates/cadx-app/assets/DroidSansFallback-LICENSE.txt).

The workspace does not yet bundle a geometry kernel. Additional exchange or
geometry dependencies must be reviewed for license, security, and platform
implications before use.
