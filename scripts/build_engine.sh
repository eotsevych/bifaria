#!/usr/bin/env bash
# Builds the Windows/Linux speech helper (src-tauri/engine: transcribe-cpp, Vulkan + CPU) and puts
# it with its libraries in src-tauri/engine-bundle, which tauri.{windows,linux}.conf.json ship as
# resources/engine. Needs the Vulkan SDK (Windows) or libvulkan-dev and glslc (Linux).
# Usage: scripts/build_engine.sh   (ENGINE_TARGET_DIR overrides cargo's build folder)
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
engine="$root/src-tauri/engine"
out="$root/src-tauri/engine-bundle"
target="${ENGINE_TARGET_DIR:-$engine/target}"

# ONNX Runtime (DirectML, Windows) links its DLLs into these; a fresh or restored build folder can lack them.
mkdir -p "$target"/release/{examples,deps} "$target"/release/build/{examples,deps}
cargo build --release --manifest-path "$engine/Cargo.toml" --target-dir "$target"

exe=bifaria-asr
[[ "${OS:-}" == Windows_NT ]] && exe=bifaria-asr.exe
rm -rf "$out"
mkdir -p "$out"
cp "$target/release/$exe" "$out/"
cp "$engine/runtime-libs/"* "$out/"
if [[ $exe == *.exe ]]; then
  # ONNX Runtime with DirectML for Parakeet's ONNX build on the GPU.
  for dll in onnxruntime.dll onnxruntime_providers_shared.dll DirectML.dll; do
    found=$(find "$target/release" -maxdepth 3 -name "$dll" | head -1)
    if [[ -n "$found" ]]; then cp "$found" "$out/"; elif [[ $dll != onnxruntime_providers_shared.dll ]]; then echo "$dll wasn't built" >&2; exit 1; fi
  done
fi

# The tests load the libraries next to the helper.
PATH="$out:$PATH" LD_LIBRARY_PATH="$out:${LD_LIBRARY_PATH:-}" \
  cargo test --release --manifest-path "$engine/Cargo.toml" --target-dir "$target"
ls "$out" | grep -q 'ggml-vulkan' || { echo "no ggml-vulkan module was built" >&2; exit 1; }
ls "$out" | grep -q 'ggml-cpu' || { echo "no ggml-cpu modules were built" >&2; exit 1; }

# OpenVINO (Intel GPU, NPU, CPU) through FluidInference's eddy, which the helper loads at run time
# (src-tauri/engine/src/openvino.rs). ENGINE_OPENVINO=0 skips it. Needs CMake, a C++20 compiler and
# vcpkg (VCPKG_INSTALLATION_ROOT, preinstalled on GitHub's runners).
if [[ "${ENGINE_OPENVINO:-1}" != 0 ]]; then
  ov_url=https://storage.openvinotoolkit.org/repositories/openvino/packages/2025.4.1
  eddy_commit=07028cf333f97244f0f3ff718cc748d7dd0a8915
  work="${ENGINE_WORK_DIR:-$engine/openvino-build}"
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*)
      os=windows triplet=x64-windows-static-md
      ov_file=windows/openvino_toolkit_windows_2025.4.1.20426.82bbf0292c5_x86_64.zip
      ov_sha=0ff9122bdd11a05f6511acb6927292e422b41ed8f3c2e25ec78d80c95a0e68c8 ;;
    Linux)
      os=linux triplet=x64-linux
      ov_file=linux/openvino_toolkit_ubuntu22_2025.4.1.20426.82bbf0292c5_x86_64.tgz
      ov_sha=4e9501655a30e93ebdd1c21944c16eb094012cc541653f8151ed981061466d84 ;;
    *) echo "OpenVINO: only Windows and Linux" >&2; exit 1 ;;
  esac
  mkdir -p "$work"
  archive="$work/$(basename "$ov_file")"
  [[ -f "$archive" ]] || curl -L --fail -o "$archive" "$ov_url/$ov_file"
  echo "$ov_sha  $archive" | sha256sum -c -
  rm -rf "$work/openvino" && mkdir -p "$work/openvino"
  if [[ $os == windows ]]; then unzip -q "$archive" -d "$work/openvino"; else tar -xzf "$archive" -C "$work/openvino"; fi
  ov=$(echo "$work"/openvino/openvino_toolkit_*)

  eddy="$work/eddy"
  rm -rf "$eddy" && git init -q "$eddy"
  git -C "$eddy" fetch -q --depth 1 https://github.com/FluidInference/eddy.git "$eddy_commit"
  git -C "$eddy" checkout -q FETCH_HEAD
  # Bifaria passes audio buffers, never files: libsndfile without its codec libraries is enough.
  printf '%s\n' '{ "name": "eddy", "version": "0.1.0", "dependencies": [ { "name": "libsndfile", "default-features": false }, "libsamplerate" ] }' > "$eddy/vcpkg.json"
  vcpkg_root="${VCPKG_INSTALLATION_ROOT:?set VCPKG_INSTALLATION_ROOT to a vcpkg checkout}"
  if [[ $os == windows ]]; then vcpkg_root=$(cygpath -m "$vcpkg_root"); ov_cmake=$(cygpath -m "$ov/runtime/cmake"); else ov_cmake="$ov/runtime/cmake"; fi
  cmake -S "$eddy" -B "$eddy/build" -DCMAKE_BUILD_TYPE=Release -DEDDY_BUILD_EXAMPLES=OFF -DEDDY_ENABLE_WHISPER=OFF \
    -DBUILD_TESTING=OFF -DOpenVINO_DIR="$ov_cmake" -DVCPKG_TARGET_TRIPLET="$triplet" \
    -DCMAKE_TOOLCHAIN_FILE="$vcpkg_root/scripts/buildsystems/vcpkg.cmake"
  cmake --build "$eddy/build" --config Release --target eddy_c --parallel

  if [[ $os == windows ]]; then
    cp "$eddy/build/Release/eddy_c.dll" "$out/"
    bin="$ov/runtime/bin/intel64/Release"
    for f in openvino openvino_c openvino_ir_frontend openvino_intel_cpu_plugin openvino_intel_gpu_plugin openvino_intel_npu_plugin; do
      cp "$bin/$f.dll" "$out/"
    done
    cp "$bin"/*.json "$out/" 2>/dev/null || true
    cp "$ov"/runtime/3rdparty/tbb/bin/tbb12.dll "$out/"
  else
    cp "$eddy/build/libeddy_c.so" "$out/"
    lib="$ov/runtime/lib/intel64"
    for f in libopenvino libopenvino_c libopenvino_ir_frontend; do cp -a "$lib/$f".so* "$out/"; done
    for f in cpu gpu npu; do cp -a "$lib/libopenvino_intel_${f}_plugin.so" "$out/"; done
    cp "$lib"/*.json "$out/" 2>/dev/null || true
    if [[ -d "$ov/runtime/3rdparty/tbb/lib" ]]; then cp -a "$ov"/runtime/3rdparty/tbb/lib/libtbb.so* "$out/"; fi
  fi
fi
ls -la "$out"
