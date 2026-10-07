#include "../vendor/openvr/openvr.h"
#include <dlfcn.h>
#include <cstdint>
#include <cstring>
static void* library = nullptr;
static vr::IVRCompositor* compositor = nullptr;
static vr::IVRApplications* applications = nullptr;
static void (*shutdown_api)() = nullptr;
extern "C" void mixer_vr_close() {
    if (shutdown_api) shutdown_api();
    compositor = nullptr; applications = nullptr; shutdown_api = nullptr;
    if (library) dlclose(library);
    library = nullptr;
}
extern "C" bool mixer_vr_open(const char* path) {
    mixer_vr_close();
    library = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (!library) return false;
    auto init = reinterpret_cast<uint32_t (*)(vr::EVRInitError*, vr::EVRApplicationType, const char*)>(dlsym(library, "VR_InitInternal2"));
    auto get = reinterpret_cast<void* (*)(const char*, vr::EVRInitError*)>(dlsym(library, "VR_GetGenericInterface"));
    auto shutdown = reinterpret_cast<void (*)()>(dlsym(library, "VR_ShutdownInternal"));
    if (!init || !get || !shutdown) { mixer_vr_close(); return false; }
    vr::EVRInitError error = vr::VRInitError_None;
    init(&error, vr::VRApplication_Background, nullptr);
    if (error != vr::VRInitError_None) { mixer_vr_close(); return false; }
    shutdown_api = shutdown;
    compositor = static_cast<vr::IVRCompositor*>(get(vr::IVRCompositor_Version, &error));
    applications = static_cast<vr::IVRApplications*>(get(vr::IVRApplications_Version, &error));
    if (!compositor || !applications) { mixer_vr_close(); return false; }
    return true;
}
extern "C" int64_t mixer_vr_pid() {
    return compositor ? compositor->GetCurrentSceneFocusProcess() : -1;
}
extern "C" bool mixer_vr_application(uint32_t pid) {
    char key[vr::k_unMaxApplicationKeyLength] = {};
    return applications && pid && applications->GetApplicationKeyByProcessId(pid, key, sizeof(key)) == vr::VRApplicationError_None;
}
extern "C" uint32_t mixer_vr_applications(uint32_t* pids, uint32_t capacity) {
    if (!applications) return 0;
    uint32_t count = 0;
    for (uint32_t i = 0; i < applications->GetApplicationCount() && count < capacity; ++i) {
        char key[vr::k_unMaxApplicationKeyLength] = {};
        if (applications->GetApplicationKeyByIndex(i, key, sizeof(key)) == vr::VRApplicationError_None) {
            // Scene games have Steam manifests; background/overlay tools must not
            // be mistaken for scene applications just because they use OpenVR.
            if (std::strncmp(key, "steam.app.", 10) != 0 ||
                applications->GetApplicationPropertyBool(key, vr::VRApplicationProperty_IsDashboardOverlay_Bool) ||
                applications->GetApplicationPropertyBool(key, vr::VRApplicationProperty_IsInternal_Bool)) continue;
            const auto pid = applications->GetApplicationProcessId(key);
            if (pid) pids[count++] = pid;
        }
    }
    return count;
}
