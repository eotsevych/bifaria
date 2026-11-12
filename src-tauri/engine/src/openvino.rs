//! Parakeet on Intel GPUs, NPUs and CPUs through OpenVINO, with FluidInference's eddy library
//! (Apache-2.0) and its OpenVINO build of Parakeet v3. Both libraries are loaded from the helper's
//! own folder only when asked, so the helper still runs where they're missing.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use libloading::Library;

/// eddy's `EddyParakeetConfig`.
#[repr(C)]
struct Config {
    device: *const c_char,
    model_dir: *const c_char,
    blank_token_id: c_int,
}

/// eddy's `EddyParakeetResult`.
#[repr(C)]
struct InferResult {
    text: *mut c_char,
    token_ids: *mut c_int,
    num_tokens: usize,
    confidence: f32,
    latency_ms: f64,
}

/// Parakeet v3's blank token, which also tells eddy the model is v3.
const V3_BLANK: c_int = 8192;

type Create = unsafe extern "C" fn(Config, *mut *mut c_char) -> *mut c_void;
type Destroy = unsafe extern "C" fn(*mut c_void);
type InferBuffer = unsafe extern "C" fn(*mut c_void, *const f32, usize, c_int, *mut InferResult, *mut *mut c_char) -> c_int;
type FreeResult = unsafe extern "C" fn(*mut InferResult);
type FreeString = unsafe extern "C" fn(*mut c_char);

/// A Parakeet model loaded on one OpenVINO device.
pub struct Eddy {
    model: *mut c_void,
    destroy: Destroy,
    infer: InferBuffer,
    free_result: FreeResult,
    free_string: FreeString,
    // Last: the functions above live in it.
    _lib: Library,
}

// eddy's handles may be used from any thread (eddy_c.h); the helper uses one at a time anyway.
unsafe impl Send for Eddy {}

fn lib_path(stem: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the helper has no folder")?;
    let name = if cfg!(windows) {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        // Linux ships each library once, under its SONAME (scripts/build_engine.sh): libopenvino_c.so.2541.
        let plain = format!("lib{stem}.so");
        std::fs::read_dir(dir)
            .ok()
            .and_then(|entries| {
                entries
                    .filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|n| *n == plain || n.starts_with(&format!("{plain}.")))
                    .min_by_key(|n| n.len())
            })
            .unwrap_or(plain)
    };
    Ok(dir.join(name))
}

fn open(stem: &str) -> Result<Library, String> {
    let path = lib_path(stem)?;
    // SAFETY: our own bundled library, loaded from the helper's folder.
    unsafe { Library::new(&path) }.map_err(|e| format!("OpenVINO isn't available ({}: {e})", path.display()))
}

/// Takes and frees an error string eddy allocated.
fn take_error(free_string: FreeString, err: *mut c_char) -> String {
    if err.is_null() {
        return "unknown error".to_string();
    }
    // SAFETY: eddy returns a NUL-terminated string we must free with eddy_free_string.
    let message = unsafe { CStr::from_ptr(err) }.to_string_lossy().into_owned();
    unsafe { free_string(err) };
    message
}

impl Eddy {
    /// Loads the OpenVINO Parakeet v3 in `dir` onto `device` ("GPU", "NPU" or "CPU"). The first
    /// load on a GPU or NPU compiles the model, which can take a while; OpenVINO caches it.
    pub fn load(dir: &Path, device: &str) -> Result<Self, String> {
        let lib = open("eddy_c")?;
        // SAFETY: symbols and signatures from eddy_c.h at the pinned eddy commit (scripts/build_engine.sh).
        unsafe {
            let create: Create = *lib.get(b"eddy_parakeet_create\0").map_err(|e| e.to_string())?;
            let destroy: Destroy = *lib.get(b"eddy_parakeet_destroy\0").map_err(|e| e.to_string())?;
            let infer: InferBuffer = *lib.get(b"eddy_parakeet_infer_buffer\0").map_err(|e| e.to_string())?;
            let free_result: FreeResult = *lib.get(b"eddy_parakeet_free_result\0").map_err(|e| e.to_string())?;
            let free_string: FreeString = *lib.get(b"eddy_free_string\0").map_err(|e| e.to_string())?;
            let device_c = CString::new(device).map_err(|e| e.to_string())?;
            let dir_c = CString::new(dir.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            let mut err: *mut c_char = std::ptr::null_mut();
            let model = create(Config { device: device_c.as_ptr(), model_dir: dir_c.as_ptr(), blank_token_id: V3_BLANK }, &mut err);
            if model.is_null() {
                return Err(take_error(free_string, err));
            }
            if !err.is_null() {
                free_string(err);
            }
            Ok(Self { model, destroy, infer, free_result, free_string, _lib: lib })
        }
    }

    /// Text for 16 kHz mono samples.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let mut out = InferResult { text: std::ptr::null_mut(), token_ids: std::ptr::null_mut(), num_tokens: 0, confidence: 0.0, latency_ms: 0.0 };
        let mut err: *mut c_char = std::ptr::null_mut();
        // SAFETY: a live model handle; `samples` outlives the call; `out` is freed below.
        let status = unsafe { (self.infer)(self.model, samples.as_ptr(), samples.len(), 16_000, &mut out, &mut err) };
        if status != 0 {
            return Err(take_error(self.free_string, err));
        }
        if !err.is_null() {
            unsafe { (self.free_string)(err) };
        }
        let text = if out.text.is_null() { String::new() } else { unsafe { CStr::from_ptr(out.text) }.to_string_lossy().into_owned() };
        unsafe { (self.free_result)(&mut out) };
        Ok(text)
    }
}

impl Drop for Eddy {
    fn drop(&mut self) {
        // SAFETY: the handle from eddy_parakeet_create, destroyed once.
        unsafe { (self.destroy)(self.model) };
    }
}

/// openvino_c's `ov_available_devices_t`.
#[repr(C)]
struct AvailableDevices {
    devices: *mut *mut c_char,
    size: usize,
}

/// OpenVINO's devices here as (id, name), e.g. ("GPU", "Intel(R) Iris(R) Xe Graphics (iGPU)").
pub fn devices() -> Result<Vec<(String, String)>, String> {
    type CoreCreate = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
    type CoreFree = unsafe extern "C" fn(*mut c_void);
    type GetDevices = unsafe extern "C" fn(*const c_void, *mut AvailableDevices) -> c_int;
    type DevicesFree = unsafe extern "C" fn(*mut AvailableDevices);
    type GetProperty = unsafe extern "C" fn(*const c_void, *const c_char, *const c_char, *mut *mut c_char) -> c_int;
    type FreeStr = unsafe extern "C" fn(*mut c_char);

    let lib = open("openvino_c")?;
    // SAFETY: OpenVINO's C API (openvino/c/*.h, 2025.4); every allocation is freed with its own function.
    unsafe {
        let core_create: CoreCreate = *lib.get(b"ov_core_create\0").map_err(|e| e.to_string())?;
        let core_free: CoreFree = *lib.get(b"ov_core_free\0").map_err(|e| e.to_string())?;
        let get_devices: GetDevices = *lib.get(b"ov_core_get_available_devices\0").map_err(|e| e.to_string())?;
        let devices_free: DevicesFree = *lib.get(b"ov_available_devices_free\0").map_err(|e| e.to_string())?;
        let get_property: GetProperty = *lib.get(b"ov_core_get_property\0").map_err(|e| e.to_string())?;
        let free_str: FreeStr = *lib.get(b"ov_free\0").map_err(|e| e.to_string())?;

        let mut core: *mut c_void = std::ptr::null_mut();
        if core_create(&mut core) != 0 || core.is_null() {
            return Err("OpenVINO couldn't start".to_string());
        }
        let mut list = AvailableDevices { devices: std::ptr::null_mut(), size: 0 };
        let mut out = Vec::new();
        if get_devices(core, &mut list) == 0 {
            for i in 0..list.size {
                let id_ptr = *list.devices.add(i);
                let id = CStr::from_ptr(id_ptr).to_string_lossy().into_owned();
                let mut name_ptr: *mut c_char = std::ptr::null_mut();
                let name = if get_property(core, id_ptr, c"FULL_DEVICE_NAME".as_ptr(), &mut name_ptr) == 0 && !name_ptr.is_null() {
                    let name = CStr::from_ptr(name_ptr).to_string_lossy().into_owned();
                    free_str(name_ptr);
                    name
                } else {
                    id.clone()
                };
                out.push((id, name));
            }
            devices_free(&mut list);
        }
        core_free(core);
        Ok(out)
    }
}
