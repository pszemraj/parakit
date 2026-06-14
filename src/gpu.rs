//! Bundled ggml device enumeration.

use crate::ffi_util::c_string_lossy;
use std::os::raw::{c_char, c_int};

type GgmlBackendDev = *mut GgmlBackendDevice;

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
        out.push(DeviceInfo {
            name: device_ref
                .iface
                .get_name
                .map(|get_name| c_string_lossy(unsafe { get_name(device) }))
                .unwrap_or_default(),
            description: device_ref
                .iface
                .get_description
                .map(|get_description| c_string_lossy(unsafe { get_description(device) }))
                .unwrap_or_default(),
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

/// Return the GPU device ggml is expected to prefer.
///
/// # Returns
///
/// The first discrete GPU when one is visible, otherwise the first integrated
/// GPU. Returns `None` when ggml reports no GPU or iGPU devices.
pub fn preferred_gpu_device() -> Option<DeviceInfo> {
    let devices = devices();
    preferred_gpu_device_in(&devices).cloned()
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
    (free_bytes, total_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ggml_device_types() {
        assert_eq!(DeviceKind::from(0), DeviceKind::Cpu);
        assert_eq!(DeviceKind::from(1), DeviceKind::Gpu);
        assert_eq!(DeviceKind::from(2), DeviceKind::IGpu);
        assert_eq!(DeviceKind::from(9), DeviceKind::Unknown(9));
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

    #[test]
    fn preferred_gpu_device_prefers_discrete_over_integrated() {
        let devices = vec![
            DeviceInfo {
                name: "Vulkan0".to_string(),
                description: "AMD 780M".to_string(),
                kind: DeviceKind::IGpu,
                free_bytes: 0,
                total_bytes: 0,
            },
            DeviceInfo {
                name: "Vulkan1".to_string(),
                description: "NVIDIA RTX".to_string(),
                kind: DeviceKind::Gpu,
                free_bytes: 0,
                total_bytes: 0,
            },
        ];

        assert_eq!(
            preferred_gpu_device_in(&devices).map(|device| device.name.as_str()),
            Some("Vulkan1")
        );
    }

    #[test]
    fn preferred_gpu_device_uses_integrated_when_no_discrete_gpu_exists() {
        let devices = vec![
            DeviceInfo {
                name: "BLAS".to_string(),
                description: "OpenBLAS".to_string(),
                kind: DeviceKind::Accel,
                free_bytes: 0,
                total_bytes: 0,
            },
            DeviceInfo {
                name: "Vulkan0".to_string(),
                description: "AMD 780M".to_string(),
                kind: DeviceKind::IGpu,
                free_bytes: 0,
                total_bytes: 0,
            },
        ];

        assert_eq!(
            preferred_gpu_device_in(&devices).map(|device| device.name.as_str()),
            Some("Vulkan0")
        );
    }
}
