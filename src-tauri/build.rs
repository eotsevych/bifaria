use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
  // bifaria-translate: the helper for the "Apple" translation provider. Universal; needs the macOS 26 SDK.
  build_helper(Helper {
    name: "bifaria-translate",
    inputs: &["apple-translate/bifaria-translate.swift"],
    script: "../scripts/build_apple_translate.sh",
    require_env: "BIFARIA_REQUIRE_APPLE_TRANSLATE",
    skip_env: None,
    needs: "Xcode with the macOS 26 SDK",
    missing: "the Apple translation provider won't work in this build",
  });
  // bifaria-asr: runs Parakeet on the Neural Engine (Apple Silicon). Needs Swift 6 and, on the
  // first build, network access to fetch FluidAudio. Without it Bifaria runs Parakeet on the CPU.
  build_helper(Helper {
    name: "bifaria-asr",
    inputs: &["apple-asr/Package.swift", "apple-asr/Sources/main.swift"],
    script: "../scripts/build_apple_asr.sh",
    require_env: "BIFARIA_REQUIRE_APPLE_ASR",
    skip_env: Some("BIFARIA_SKIP_APPLE_ASR"),
    needs: "Xcode with Swift 6 and network access for FluidAudio",
    missing: "Parakeet will run on the CPU instead of the Neural Engine",
  });
  stand_in_engine_bundle();
  tauri_build::build()
}

/// tauri.{windows,linux}.conf.json bundle `engine-bundle/*`, which scripts/build_engine.sh fills
/// (CI does). Without it, a placeholder keeps the build going; Bifaria then offers no Vulkan.
fn stand_in_engine_bundle() {
  let target = std::env::var("TARGET").unwrap_or_default();
  let dir = Path::new("engine-bundle");
  if target.ends_with("apple-darwin") || dir.exists() {
    return;
  }
  println!("cargo:warning=No speech helper in src-tauri/engine-bundle (run scripts/build_engine.sh); Parakeet won't offer Vulkan.");
  std::fs::create_dir_all(dir).expect("create engine-bundle");
  std::fs::write(dir.join("README.txt"), "Run scripts/build_engine.sh to build the speech helper.\n").expect("write engine-bundle/README.txt");
}

struct Helper {
  name: &'static str,
  inputs: &'static [&'static str],
  script: &'static str,
  /// Set (release builds do) to fail the build instead of going on with a stand-in.
  require_env: &'static str,
  /// Set to use the stand-in without trying to build (e.g. offline).
  skip_env: Option<&'static str>,
  needs: &'static str,
  missing: &'static str,
}

/// Builds a Swift helper that tauri.macos.conf.json bundles as a sidecar
/// (`binaries/<name>-<target triple>`). One build serves every macOS target.
///
/// When it can't be built, the build goes on with a stand-in that answers every request with an
/// error (Bifaria treats it as missing), unless `require_env` is set.
fn build_helper(helper: Helper) {
  let target = std::env::var("TARGET").unwrap_or_default();
  if !target.ends_with("apple-darwin") {
    return;
  }
  let script = Path::new(helper.script);
  let mut inputs: Vec<&Path> = helper.inputs.iter().map(Path::new).collect();
  inputs.push(script);
  for input in &inputs {
    println!("cargo:rerun-if-changed={}", input.display());
  }
  println!("cargo:rerun-if-env-changed={}", helper.require_env);
  let skip = helper.skip_env.is_some_and(|env| {
    println!("cargo:rerun-if-env-changed={env}");
    std::env::var_os(env).is_some()
  });

  let universal = PathBuf::from(format!("binaries/{}-universal-apple-darwin", helper.name));
  let sidecar = PathBuf::from(format!("binaries/{}-{}", helper.name, target));
  if skip {
    if !universal.exists() {
      write_stand_in(&universal, helper.name);
    }
  } else if !is_newer(&universal, &inputs) || is_stand_in(&universal) {
    let built = Command::new("bash").arg(script).arg(&universal).status().map(|s| s.success()).unwrap_or(false);
    if !built {
      if std::env::var_os(helper.require_env).is_some() {
        panic!("Couldn't build {} ({}); it needs {}", helper.name, helper.script, helper.needs);
      }
      println!("cargo:warning=Couldn't build {} (needs {}); {}.", helper.name, helper.needs, helper.missing);
      write_stand_in(&universal, helper.name);
    }
  }
  if sidecar != universal {
    std::fs::copy(&universal, &sidecar).expect("copy the helper");
  }
}

const STAND_IN_MARK: &str = "stand-in";

/// Answers every request (and a download) with an error saying why the helper is missing.
fn write_stand_in(path: &Path, name: &str) {
  let message = format!("This Bifaria build has no {name} helper (it couldn't be built)");
  let script = format!(
    "#!/bin/sh\n# {name} {STAND_IN_MARK}\nanswer='{{\"ok\":false,\"error\":\"internal\",\"message\":\"{message}\"}}'\nif [ \"$1\" = download ]; then echo \"$answer\"; exit 1; fi\nwhile read -r _; do echo \"$answer\"; done\n"
  );
  std::fs::create_dir_all(path.parent().unwrap()).expect("create src-tauri/binaries");
  std::fs::write(path, script).expect("write the helper stand-in");
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
  }
}

fn is_stand_in(path: &Path) -> bool {
  std::fs::read(path).map(|b| b.starts_with(b"#!") && String::from_utf8_lossy(&b).contains(STAND_IN_MARK)).unwrap_or(false)
}

/// True when `path` exists and is newer than every input.
fn is_newer(path: &Path, inputs: &[&Path]) -> bool {
  let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
  let Some(built) = modified(path) else { return false };
  inputs.iter().all(|input| modified(input).map_or(true, |m| m <= built))
}
