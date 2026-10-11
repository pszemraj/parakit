// Compile against the same ggml source headers as the linked CrispASR build.
#include "ggml-backend-impl.h"

extern "C" {

// Parakit-owned ABI: strings are borrowed from the registered ggml device.
struct ParakitGgmlDeviceInfo {
    const char * name;
    const char * description;
    const char * device_id;
    size_t free_bytes;
    size_t total_bytes;
    int kind;
};

size_t parakit_ggml_device_count() {
    return ggml_backend_dev_count();
}

int parakit_ggml_device_get(size_t index, ParakitGgmlDeviceInfo * info) {
    const ggml_backend_dev_t device = ggml_backend_dev_get(index);
    if (device == nullptr) {
        return 0;
    }

    *info = {};
    info->kind = -1;
    // Some Windows import libraries omit the public property accessors.
    // Reading callbacks here makes the native compiler own their layout.
    if (device->iface.get_name != nullptr) {
        info->name = device->iface.get_name(device);
    }
    if (device->iface.get_description != nullptr) {
        info->description = device->iface.get_description(device);
    }
    if (device->iface.get_memory != nullptr) {
        device->iface.get_memory(device, &info->free_bytes, &info->total_bytes);
    }
    if (device->iface.get_type != nullptr) {
        info->kind = static_cast<int>(device->iface.get_type(device));
    }
#ifdef _WIN32
    if (device->iface.get_props != nullptr) {
        ggml_backend_dev_props props = {};
        device->iface.get_props(device, &props);
        info->device_id = props.device_id;
    }
#endif
    return 1;
}

}
