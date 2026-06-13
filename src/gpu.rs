//! Bundled ggml device enumeration.

use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

type GgmlBackendDev = *mut c_void;

extern "C" {
    fn ggml_backend_dev_count() -> usize;
    fn ggml_backend_dev_get(index: usize) -> GgmlBackendDev;
    fn ggml_backend_dev_name(device: GgmlBackendDev) -> *const c_char;
    fn ggml_backend_dev_description(device: GgmlBackendDev) -> *const c_char;
    fn ggml_backend_dev_memory(device: GgmlBackendDev, free: *mut usize, total: *mut usize);
    fn ggml_backend_dev_type(device: GgmlBackendDev) -> c_int;
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
    pub fn is_gpu_like(self) -> bool {
        matches!(self, Self::Gpu | Self::IGpu)
    }

    /// Stable diagnostic label.
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
    pub fn is_gpu_like(&self) -> bool {
        self.kind.is_gpu_like()
    }

    /// Format a concise diagnostic line.
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
pub fn devices() -> Vec<DeviceInfo> {
    let count = unsafe { ggml_backend_dev_count() };
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let device = unsafe { ggml_backend_dev_get(index) };
        if device.is_null() {
            continue;
        }
        let mut free_bytes = 0;
        let mut total_bytes = 0;
        unsafe {
            ggml_backend_dev_memory(device, &mut free_bytes, &mut total_bytes);
        }
        out.push(DeviceInfo {
            name: c_string(unsafe { ggml_backend_dev_name(device) }),
            description: c_string(unsafe { ggml_backend_dev_description(device) }),
            kind: DeviceKind::from(unsafe { ggml_backend_dev_type(device) }),
            free_bytes,
            total_bytes,
        });
    }
    out
}

/// Return whether any discrete or integrated GPU is visible to ggml.
pub fn has_gpu_device() -> bool {
    devices().iter().any(DeviceInfo::is_gpu_like)
}

fn bytes_to_mib(bytes: usize) -> usize {
    bytes / 1_048_576
}

fn c_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
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
}
