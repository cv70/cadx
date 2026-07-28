# Third-Party Notices

CADX currently uses the following third-party crates:

| Dependency | License | Purpose |
| --- | --- | --- |
| `serde` | MIT OR Apache-2.0 | Versioned document and history serialization contracts. |
| `eframe` / `egui` | MIT OR Apache-2.0 | Native desktop workbench shell. |

The initial workspace does not bundle import/export adapters, geometry kernels,
network clients, or AI providers. Additions must be reviewed for license and
platform implications before use.
