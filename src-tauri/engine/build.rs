//! Copies transcribe-cpp's runtime libraries (the core DLLs and, with `dynamic-backends`, the
//! per-CPU and Vulkan backend modules) into `runtime-libs/`, to ship next to the helper: without
//! the modules `init_backends_default()` finds no devices. A static build (macOS Metal) has none.

use std::path::PathBuf;

fn main() {
    let dest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("runtime-libs");
    let _ = std::fs::remove_dir_all(&dest);
    for var in ["DEP_TRANSCRIBE_CPP_RUNTIME_DIR", "DEP_TRANSCRIBE_CPP_MODULE_DIR"] {
        println!("cargo:rerun-if-env-changed={var}");
        let Some(dir) = std::env::var_os(var) else { continue };
        std::fs::create_dir_all(&dest).expect("create runtime-libs");
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")).flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".dll") || name.ends_with(".dylib") || name.contains(".so") {
                std::fs::copy(entry.path(), dest.join(&name)).unwrap_or_else(|e| panic!("copy {name}: {e}"));
            }
        }
    }
}
