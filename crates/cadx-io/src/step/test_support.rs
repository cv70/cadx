//! Fixtures shared by the STEP adapter's unit tests.

use std::sync::atomic::{AtomicU64, Ordering};

use super::{StepImport, read_step};

pub(super) const VALID_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('CADX'),'2;1');\nFILE_NAME('model.step','',(''),(''),'CADX','CADX','');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\nENDSEC;\nEND-ISO-10303-21;\n";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

pub(super) fn read_source(source: &str) -> StepImport {
    let path = std::env::temp_dir().join(format!(
        "cadx-step-import-{}-{}-{}.step",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, source).unwrap();
    let imported = read_step(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    imported
}
