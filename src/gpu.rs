//! Bundled ggml device enumeration.

use crate::ffi_util::c_string_lossy;
use std::os::raw::{c_char, c_int};

type GgmlBackendDev = *mut GgmlBackendDevice;

// Prefix mirror of ggml's `ggml_backend_device` and leading
// `ggml_backend_device_i` fields. The public header declares accessors for
// these fields, but the pinned Windows import library does not export them.
// Keep this in sync with vendor/CrispASR/ggml/src/ggml-backend-impl.h through
// `get_props`.
#[repr(C)]
struct GgmlBackendDevice {
    iface: GgmlBackendDeviceIface,
}

#[repr(C)]
struct GgmlBackendDeviceIface {
    get_name: Option<unsafe extern "C" fn(GgmlBackendDev) -> *const c_char>,
    get_description: Option<unsafe extern "C" fn(GgmlBackendDev) -> *const c_char>,
    get_memory: Option<unsafe extern "C" fn(GgmlBackendDev, *mut usize, *mut usize)>,
    get_type: Option<unsafe extern "C" fn(GgmlBackendDev) -> c_int>,
    #[cfg(any(target_os = "windows", test))]
    get_props: Option<unsafe extern "C" fn(GgmlBackendDev, *mut GgmlBackendDeviceProps)>,
}

// Mirror ggml_backend_dev_props, including all four ggml_backend_dev_caps
// bool fields. get_props writes the whole structure, not just device_id.
#[cfg(any(target_os = "windows", test))]
#[repr(C)]
struct GgmlBackendDeviceProps {
    name: *const c_char,
    description: *const c_char,
    memory_free: usize,
    memory_total: usize,
    kind: c_int,
    device_id: *const c_char,
    caps: [bool; 4],
}

extern "C" {
    fn ggml_backend_dev_count() -> usize;
    fn ggml_backend_dev_get(index: usize) -> GgmlBackendDev;
}

/// ggml compute device class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceKind {
    Cpu,
    Gpu,
    IGpu,
    Accel,
    Meta,
    Unknown(c_int),
}

impl DeviceKind {
    /// Return whether this device is a discrete or integrated GPU.
    ///
    /// # Returns
    ///
    /// `true` for ggml discrete GPU and integrated GPU device classes.
    pub fn is_gpu_like(self) -> bool {
        matches!(self, Self::Gpu | Self::IGpu)
    }

    /// Stable diagnostic label.
    ///
    /// # Returns
    ///
    /// A short label suitable for `doctor` output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
            Self::IGpu => "iGPU",
            Self::Accel => "Accel",
            Self::Meta => "Meta",
            Self::Unknown(_) => "Unknown",
        }
    }
}

impl From<c_int> for DeviceKind {
    fn from(value: c_int) -> Self {
        match value {
            0 => Self::Cpu,
            1 => Self::Gpu,
            2 => Self::IGpu,
            3 => Self::Accel,
            4 => Self::Meta,
            other => Self::Unknown(other),
        }
    }
}

/// Runtime ggml device info.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub description: String,
    pub kind: DeviceKind,
    pub free_bytes: usize,
    pub total_bytes: usize,
}

impl DeviceInfo {
    /// Return whether this device is a discrete or integrated GPU.
    ///
    /// # Returns
    ///
    /// `true` for discrete GPU and integrated GPU devices.
    pub fn is_gpu_like(&self) -> bool {
        self.kind.is_gpu_like()
    }

    /// Format a concise diagnostic line.
    ///
    /// # Returns
    ///
    /// A human-readable single-line device summary.
    pub fn diagnostic_line(&self) -> String {
        let description = if self.description.is_empty() || self.description == self.name {
            String::new()
        } else {
            format!(" - {}", self.description)
        };
        let memory = if self.total_bytes == 0 {
            String::new()
        } else {
            format!(
                " ({} MiB free / {} MiB total)",
                bytes_to_mib(self.free_bytes),
                bytes_to_mib(self.total_bytes)
            )
        };
        format!(
            "{}{} [{}]{}",
            self.name,
            description,
            self.kind.label(),
            memory
        )
    }
}

/// Enumerate ggml compute devices registered in the bundled library.
///
/// # Returns
///
/// Device information for each non-null ggml device handle.
pub fn devices() -> Vec<DeviceInfo> {
    let count = unsafe { ggml_backend_dev_count() };
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let device = unsafe { ggml_backend_dev_get(index) };
        if device.is_null() {
            continue;
        }
        let device_ref = unsafe { &*device };
        let (free_bytes, total_bytes) = device_memory(device, device_ref);
        let name = device_ref
            .iface
            .get_name
            .map(|get_name| c_string_lossy(unsafe { get_name(device) }))
            .unwrap_or_default();
        let description = device_ref
            .iface
            .get_description
            .map(|get_description| c_string_lossy(unsafe { get_description(device) }))
            .unwrap_or_default();
        #[cfg(target_os = "windows")]
        let free_bytes = if description.starts_with("NVIDIA") {
            // CUDA's WDDM reading does not include other processes' allocations.
            // The driver's device-wide reading also covers NVIDIA Vulkan.
            device_pci_bus_id(device, device_ref)
                .and_then(|id| windows_nvidia_free_memory(&id))
                .map_or(free_bytes, |available| free_bytes.min(available))
        } else {
            free_bytes
        };
        out.push(DeviceInfo {
            name,
            description,
            kind: DeviceKind::from(
                device_ref
                    .iface
                    .get_type
                    .map(|get_type| unsafe { get_type(device) })
                    .unwrap_or(-1),
            ),
            free_bytes,
            total_bytes,
        });
    }
    out
}

/// Return whether any discrete or integrated GPU is visible to ggml.
///
/// # Returns
///
/// `true` when `devices()` contains at least one GPU or iGPU.
pub fn has_gpu_device() -> bool {
    devices().iter().any(DeviceInfo::is_gpu_like)
}

/// Return the preferred GPU device from an existing device list.
///
/// # Arguments
///
/// * `devices` - Device list returned by [`devices`].
///
/// # Returns
///
/// The first discrete GPU when one is present, otherwise the first integrated
/// GPU.
pub fn preferred_gpu_device_in(devices: &[DeviceInfo]) -> Option<&DeviceInfo> {
    devices
        .iter()
        .find(|device| device.kind == DeviceKind::Gpu)
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.kind == DeviceKind::IGpu)
        })
}

fn bytes_to_mib(bytes: usize) -> usize {
    bytes / 1_048_576
}

fn device_memory(device: GgmlBackendDev, device_ref: &GgmlBackendDevice) -> (usize, usize) {
    let mut free_bytes = 0;
    let mut total_bytes = 0;
    if let Some(get_memory) = device_ref.iface.get_memory {
        unsafe {
            get_memory(device, &mut free_bytes, &mut total_bytes);
        }
    }
    normalize_device_memory(free_bytes, total_bytes)
}

fn normalize_device_memory(free_bytes: usize, total_bytes: usize) -> (usize, usize) {
    // Metal's unsigned budget-minus-allocation subtraction can wrap when a
    // process exceeds its recommended working set. Treat that as no room.
    if total_bytes > 0 && free_bytes > total_bytes {
        (0, total_bytes)
    } else {
        (free_bytes, total_bytes)
    }
}

#[cfg(any(target_os = "windows", test))]
fn device_pci_bus_id(
    device: GgmlBackendDev,
    device_ref: &GgmlBackendDevice,
) -> Option<std::ffi::CString> {
    let get_props = device_ref.iface.get_props?;
    let mut props: GgmlBackendDeviceProps = unsafe { std::mem::zeroed() };
    // SAFETY: The callback and full output layout match the pinned ggml ABI.
    unsafe { get_props(device, &mut props) };
    if props.device_id.is_null() {
        return None;
    }
    // ggml owns this NUL-terminated ID. Copy it before the next native call.
    let id = unsafe { std::ffi::CStr::from_ptr(props.device_id) };
    (!id.is_empty()).then(|| id.to_owned())
}

/// Read device-wide NVIDIA free memory for the selected physical adapter.
///
/// # Returns
///
/// Free bytes for the PCI bus ID, or none when the driver probe is unavailable.
#[cfg(target_os = "windows")]
fn windows_nvidia_free_memory(pci_bus_id: &std::ffi::CStr) -> Option<usize> {
    use std::ffi::c_void;
    use windows::core::{s, w};
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    type Proc = unsafe extern "system" fn() -> isize;
    type Init = unsafe extern "C" fn() -> u32;
    type Device = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> u32;
    type Memory = unsafe extern "C" fn(*mut c_void, *mut NvmlMemory) -> u32;

    #[repr(C)]
    struct NvmlMemory {
        total: u64,
        free: u64,
        used: u64,
    }

    struct Library(HMODULE);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                let _ = FreeLibrary(self.0);
            }
        }
    }

    // NVML ships with the NVIDIA display driver; no import library or toolkit
    // is required at runtime. Keep the module alive through every native call.
    let library = Library(unsafe { LoadLibraryW(w!("nvml.dll")) }.ok()?);
    // SAFETY: These signatures match the driver's public NVML C API.
    let (init, shutdown, device, memory) = unsafe {
        (
            std::mem::transmute::<Proc, Init>(GetProcAddress(library.0, s!("nvmlInit_v2"))?),
            std::mem::transmute::<Proc, Init>(GetProcAddress(library.0, s!("nvmlShutdown"))?),
            std::mem::transmute::<Proc, Device>(GetProcAddress(
                library.0,
                s!("nvmlDeviceGetHandleByPciBusId_v2"),
            )?),
            std::mem::transmute::<Proc, Memory>(GetProcAddress(
                library.0,
                s!("nvmlDeviceGetMemoryInfo"),
            )?),
        )
    };
    if unsafe { init() } != 0 {
        return None;
    }
    let available = (|| {
        let mut handle = std::ptr::null_mut();
        if unsafe { device(pci_bus_id.as_ptr(), &mut handle) } != 0 {
            return None;
        }
        let mut info = NvmlMemory {
            total: 0,
            free: 0,
            used: 0,
        };
        if unsafe { memory(handle, &mut info) } == 0 {
            return usize::try_from(info.free).ok();
        }
        None
    })();
    unsafe { shutdown() };
    available
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_gpu_descriptions_keep_their_own_pci_bus_ids() {
        use std::ffi::CStr;

        #[repr(C)]
        struct FakeDevice {
            backend: GgmlBackendDevice,
            pci_bus_id: *const c_char,
        }

        unsafe extern "C" fn get_props(device: GgmlBackendDev, props: *mut GgmlBackendDeviceProps) {
            let device = unsafe { &*device.cast::<FakeDevice>() };
            unsafe {
                (*props).description = c"NVIDIA RTX".as_ptr();
                (*props).device_id = device.pci_bus_id;
            }
        }

        for (id, expected) in [
            (Some(c"0000:01:00.0"), Some(c"0000:01:00.0")),
            (Some(c"0000:07:00.0"), Some(c"0000:07:00.0")),
            (Some(c""), None),
            (None, None),
        ] {
            let mut device = FakeDevice {
                backend: GgmlBackendDevice {
                    iface: GgmlBackendDeviceIface {
                        get_name: None,
                        get_description: None,
                        get_memory: None,
                        get_type: None,
                        get_props: Some(get_props),
                    },
                },
                pci_bus_id: id.map_or(std::ptr::null(), CStr::as_ptr),
            };
            let handle = &mut device.backend as GgmlBackendDev;
            assert_eq!(
                device_pci_bus_id(handle, &device.backend).as_deref(),
                expected
            );
            device.backend.iface.get_props = None;
            assert_eq!(device_pci_bus_id(handle, &device.backend), None);
        }
    }

    #[test]
    fn over_budget_device_memory_does_not_wrap_into_available_space() {
        assert_eq!(normalize_device_memory(usize::MAX - 1, 1024), (0, 1024));
        assert_eq!(normalize_device_memory(512, 1024), (512, 1024));
        assert_eq!(normalize_device_memory(0, 0), (0, 0));
    }

    #[test]
    fn maps_ggml_device_types() {
        // Codes 3 (Accel) and 4 (Meta) were previously untested; the
        // `From<c_int>` impl (gpu.rs:69-79) maps all six DeviceKind cases:
        // 0-4 explicitly, and anything else falls through to Unknown.
        for (code, expected) in [
            (0, DeviceKind::Cpu),
            (1, DeviceKind::Gpu),
            (2, DeviceKind::IGpu),
            (3, DeviceKind::Accel), // previously uncovered
            (4, DeviceKind::Meta),  // previously uncovered
            (9, DeviceKind::Unknown(9)),
        ] {
            assert_eq!(DeviceKind::from(code), expected, "code {code}");
        }
    }

    #[test]
    fn formats_memory_when_reported() {
        let info = DeviceInfo {
            name: "Vulkan0".to_string(),
            description: "NVIDIA RTX".to_string(),
            kind: DeviceKind::Gpu,
            free_bytes: 7 * 1_048_576,
            total_bytes: 8 * 1_048_576,
        };

        assert_eq!(
            info.diagnostic_line(),
            "Vulkan0 - NVIDIA RTX [GPU] (7 MiB free / 8 MiB total)"
        );
    }

    /// Build a dummy device for [`preferred_gpu_device_matrix`]. Description,
    /// free, and total bytes are never asserted by that test, so every device
    /// shares the same placeholder values.
    fn device(name: &'static str, kind: DeviceKind) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            description: String::new(),
            kind,
            free_bytes: 0,
            total_bytes: 0,
        }
    }

    struct PreferredGpuCase {
        label: &'static str,
        devices: Vec<DeviceInfo>,
        expect_name: Option<&'static str>,
    }

    #[test]
    fn preferred_gpu_device_matrix() {
        let cases = vec![
            PreferredGpuCase {
                label: "discrete preferred over integrated",
                devices: vec![
                    device("Vulkan0", DeviceKind::IGpu),
                    device("Vulkan1", DeviceKind::Gpu),
                ],
                expect_name: Some("Vulkan1"),
            },
            PreferredGpuCase {
                label: "integrated used when no discrete gpu exists",
                devices: vec![
                    device("BLAS", DeviceKind::Accel),
                    device("Vulkan0", DeviceKind::IGpu),
                ],
                expect_name: Some("Vulkan0"),
            },
            PreferredGpuCase {
                // New coverage: preferred_gpu_device_in's final `None` path
                // (gpu.rs:191, falling off both `find` calls) was previously
                // never exercised by a test.
                label: "no gpu-like device present returns None",
                devices: vec![device("BLAS", DeviceKind::Accel)],
                expect_name: None,
            },
        ];

        for case in cases {
            assert_eq!(
                preferred_gpu_device_in(&case.devices).map(|device| device.name.as_str()),
                case.expect_name,
                "{}",
                case.label
            );
        }
    }
}
