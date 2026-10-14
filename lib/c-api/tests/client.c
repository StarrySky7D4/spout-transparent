#define COBJMACROS
#define CINTERFACE
#define D3D11_NO_HELPERS
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d11.h>
#include <dxgi1_2.h>
#include <objbase.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Verify that the header protects its layout from the caller's packing mode. */
#pragma pack(push, 1)
#include "spout_transparent.h"
#pragma pack(pop)
#ifdef __cplusplus
# define STATIC_ASSERT static_assert
# define IID_ARG(value) (value)
#else
# define STATIC_ASSERT _Static_assert
# define IID_ARG(value) (&(value))
#endif
STATIC_ASSERT(sizeof(spt_sender_info) == 40, "sender info size");
STATIC_ASSERT(offsetof(spt_sender_info, generation) == 32, "generation alignment");
STATIC_ASSERT(sizeof(spt_rect) == 16, "rect size");
STATIC_ASSERT(sizeof(spt_layout) == 36, "layout size");

#define CHECK(expr) do { if (!(expr)) { \
    fprintf(stderr, "FAILED %s:%d: %s (status=%d native=%ld: %s)\n", __FILE__, __LINE__, #expr, \
        (int)spt_last_error_code(), (long)spt_last_native_error(), spt_last_error_message()); exit(1); } } while (0)
#define CALL(expr) CHECK((expr) == SPT_OK)
#define HR(expr) CHECK((expr) == S_OK)

typedef struct fixture {
    char name[128];
    HANDLE mapping, mutex, count;
    uint8_t *metadata;
    ID3D11Device *device;
    ID3D11DeviceContext *context;
    ID3D11Texture2D *texture;
    IDXGIKeyedMutex *keyed;
} fixture;

static void put_u32(uint8_t *bytes, size_t offset, uint32_t value) {
    memcpy(bytes + offset, &value, sizeof(value));
}

static void fixture_create(fixture *f, int gpu) {
    char object_name[192];
    uint32_t share_handle = 7;
    memset(f, 0, sizeof(*f));
    CHECK(sprintf_s(f->name, sizeof(f->name), "spt-abi-fixture-%lu", (unsigned long)GetCurrentProcessId()) > 0);
    if (gpu) {
        D3D_FEATURE_LEVEL level;
        D3D_FEATURE_LEVEL requested = D3D_FEATURE_LEVEL_11_0;
        D3D11_TEXTURE2D_DESC desc;
        D3D11_SUBRESOURCE_DATA initial;
        uint8_t pixels[16 * 16 * 4];
        IDXGIResource *resource = NULL;
        HANDLE shared = NULL;
        HRESULT result = D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_HARDWARE, NULL, 0, &requested, 1,
            D3D11_SDK_VERSION, &f->device, &level, &f->context);
        if (FAILED(result)) {
            result = D3D11CreateDevice(NULL, D3D_DRIVER_TYPE_WARP, NULL, 0, &requested, 1,
                D3D11_SDK_VERSION, &f->device, &level, &f->context);
        }
        CHECK(SUCCEEDED(result));
        memset(&desc, 0, sizeof(desc));
        desc.Width = 16; desc.Height = 16; desc.MipLevels = 1; desc.ArraySize = 1;
        desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        desc.SampleDesc.Count = 1;
        desc.Usage = D3D11_USAGE_DEFAULT;
        desc.BindFlags = D3D11_BIND_SHADER_RESOURCE;
        desc.MiscFlags = D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX;
        memset(pixels, 255, sizeof(pixels));
        memset(&initial, 0, sizeof(initial));
        initial.pSysMem = pixels; initial.SysMemPitch = 16 * 4;
        HR(ID3D11Device_CreateTexture2D(f->device, &desc, &initial, &f->texture));
        HR(ID3D11Texture2D_QueryInterface(f->texture, IID_ARG(IID_IDXGIResource), (void **)&resource));
        HR(IDXGIResource_GetSharedHandle(resource, &shared));
        share_handle = (uint32_t)(uintptr_t)shared;
        IDXGIResource_Release(resource);
        HR(ID3D11Texture2D_QueryInterface(f->texture, IID_ARG(IID_IDXGIKeyedMutex), (void **)&f->keyed));
        ID3D11DeviceContext_Flush(f->context);
    }
    CHECK(sprintf_s(object_name, sizeof(object_name), "%s_mutex", f->name) > 0);
    f->mutex = CreateMutexA(NULL, TRUE, object_name);
    CHECK(f->mutex != NULL);
    f->mapping = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_READWRITE, 0, 280, f->name);
    CHECK(f->mapping != NULL && GetLastError() != ERROR_ALREADY_EXISTS);
    f->metadata = (uint8_t *)MapViewOfFile(f->mapping, FILE_MAP_WRITE, 0, 0, 280);
    CHECK(f->metadata != NULL);
    memset(f->metadata, 0, 280);
    put_u32(f->metadata, 0, share_handle); put_u32(f->metadata, 4, 16); put_u32(f->metadata, 8, 16);
    put_u32(f->metadata, 12, 87); put_u32(f->metadata, 16, 3); put_u32(f->metadata, 276, 1);
    memcpy(f->metadata + 20, "C ABI fixture", 14);
    CHECK(ReleaseMutex(f->mutex));
    CHECK(sprintf_s(object_name, sizeof(object_name), "%s_Count_Semaphore", f->name) > 0);
    f->count = CreateSemaphoreA(NULL, 5, 0x7fffffff, object_name);
    CHECK(f->count != NULL);
    /* Deliberately do NOT modify SpoutSenderNames or any live sender. */
}

static void fixture_disconnect(fixture *f) {
    if (f->metadata != NULL) { CHECK(UnmapViewOfFile(f->metadata)); f->metadata = NULL; }
    if (f->mapping != NULL) { CHECK(CloseHandle(f->mapping)); f->mapping = NULL; }
}
static void fixture_destroy(fixture *f) {
    fixture_disconnect(f);
    if (f->keyed != NULL) IDXGIKeyedMutex_Release(f->keyed);
    if (f->texture != NULL) ID3D11Texture2D_Release(f->texture);
    if (f->context != NULL) ID3D11DeviceContext_Release(f->context);
    if (f->device != NULL) ID3D11Device_Release(f->device);
    CHECK(CloseHandle(f->count)); CHECK(CloseHandle(f->mutex));
}

typedef struct thread_probe { spt_handle receiver; int passed; } thread_probe;
static DWORD WINAPI handle_thread(void *argument) {
    thread_probe *probe = (thread_probe *)argument;
    spt_handle local = 0;
    uint32_t connected = 99;
    CHECK(spt_receiver_poll(probe->receiver, &connected) == SPT_INVALID_HANDLE);
    CHECK(spt_last_error_code() == SPT_INVALID_HANDLE);
    CALL(spt_receiver_create(&local));
    CHECK(local != probe->receiver);
    CHECK(spt_last_error_code() == SPT_OK);
    CALL(spt_release(local));
    probe->passed = 1;
    return 0;
}

typedef struct lock_probe { HANDLE mutex, ready, release; } lock_probe;
static DWORD WINAPI mutex_thread(void *argument) {
    lock_probe *probe = (lock_probe *)argument;
    CHECK(WaitForSingleObject(probe->mutex, 2000) == WAIT_OBJECT_0);
    CHECK(SetEvent(probe->ready));
    CHECK(WaitForSingleObject(probe->release, 2000) == WAIT_OBJECT_0);
    CHECK(ReleaseMutex(probe->mutex));
    return 0;
}

static spt_handle test_metadata(fixture *f) {
    spt_handle receiver = 0, list = 0;
    spt_sender_info info;
    uint32_t connected = 99;
    size_t count = 0, required = 0;
    uint8_t buffer[256];
    uint64_t delay = 0;
    HANDLE worker;
    thread_probe probe;
    lock_probe lock;
    struct extended_info { spt_sender_info info; uint64_t sentinel; } extended;
    CALL(spt_release(0));
    CHECK(spt_abi_version() == SPT_ABI_VERSION);
    CHECK(spt_receiver_create(NULL) == SPT_INVALID_ARGUMENT);
    CALL(spt_receiver_create(&receiver)); CHECK(receiver != 0);
    CHECK(spt_sender_list_count(receiver, &count) == SPT_INVALID_HANDLE);
    CALL(spt_receiver_poll(receiver, &connected)); CHECK(connected == 0);
    CALL(spt_receiver_next_poll_ms(receiver, &delay)); CHECK(delay == SPT_NO_DEADLINE);
    memset(&info, 0, sizeof(info)); info.struct_size = (uint32_t)sizeof(info);
    CHECK(spt_receiver_info(receiver, &info) == SPT_NOT_FOUND);
    CHECK(spt_receiver_select(receiver, NULL, 0) == SPT_INVALID_ARGUMENT);
    CHECK(spt_last_error_code() == SPT_INVALID_ARGUMENT);
    CHECK(strlen(spt_last_error_message()) > 0);
    CHECK(spt_abi_version() == SPT_ABI_VERSION);
    CHECK(spt_last_error_code() == SPT_INVALID_ARGUMENT);
    probe.receiver = receiver; probe.passed = 0;
    worker = CreateThread(NULL, 0, handle_thread, &probe, 0, NULL); CHECK(worker != NULL);
    CHECK(WaitForSingleObject(worker, 5000) == WAIT_OBJECT_0); CHECK(CloseHandle(worker));
    CHECK(probe.passed && spt_last_error_code() == SPT_INVALID_ARGUMENT);
    CHECK(spt_receiver_select(receiver, (const uint8_t *)"x\0y", 3) == SPT_INVALID_ARGUMENT);
    CHECK(spt_receiver_select(receiver, (const uint8_t *)"x", SIZE_MAX) == SPT_INVALID_ARGUMENT);
    CALL(spt_receiver_select(receiver, (const uint8_t *)f->name, strlen(f->name)));
    CALL(spt_receiver_poll(receiver, &connected)); CHECK(connected == 1);
    CALL(spt_receiver_info(receiver, &info));
    CHECK(info.width == 16 && info.height == 16 && info.format == 87 && info.usage == 3);
    CHECK(info.partner_id == 1 && info.generation == 1 && info.reserved == 0);
    CHECK(spt_receiver_name(receiver, NULL, 0, &required) == SPT_BUFFER_TOO_SMALL);
    CHECK(required == strlen(f->name) + 1);
    memset(buffer, 0xa5, sizeof(buffer));
    CHECK(spt_receiver_name(receiver, buffer, required - 1, &required) == SPT_BUFFER_TOO_SMALL);
    CHECK(buffer[0] == 0xa5 && buffer[required - 1] == 0xa5);
    CALL(spt_receiver_name(receiver, buffer, sizeof(buffer), &required));
    CHECK(strcmp((const char *)buffer, f->name) == 0);
    CALL(spt_receiver_description(receiver, buffer, sizeof(buffer), &required));
    CHECK(strcmp((const char *)buffer, "C ABI fixture") == 0);
    memset(&extended, 0, sizeof(extended)); extended.info.struct_size = (uint32_t)sizeof(extended);
    extended.sentinel = UINT64_C(0x123456789abcdef0);
    CALL(spt_sender_info_read((const uint8_t *)f->name, strlen(f->name), 0, &extended.info));
    CHECK(extended.info.generation == 0 && extended.info.struct_size == 40);
    CHECK(extended.sentinel == UINT64_C(0x123456789abcdef0));
    info.struct_size = 4;
    CHECK(spt_sender_info_read((const uint8_t *)f->name, strlen(f->name), 0, &info) == SPT_INVALID_ARGUMENT);
    CHECK(spt_sender_info_read((const uint8_t *)f->name, strlen(f->name), 0, NULL) == SPT_INVALID_ARGUMENT);
    info.struct_size = (uint32_t)sizeof(info);
    lock.mutex = f->mutex; lock.ready = CreateEventW(NULL, TRUE, FALSE, NULL);
    lock.release = CreateEventW(NULL, TRUE, FALSE, NULL); CHECK(lock.ready != NULL && lock.release != NULL);
    worker = CreateThread(NULL, 0, mutex_thread, &lock, 0, NULL); CHECK(worker != NULL);
    CHECK(WaitForSingleObject(lock.ready, 2000) == WAIT_OBJECT_0);
    CHECK(spt_sender_info_read((const uint8_t *)f->name, strlen(f->name), 0, &info) == SPT_WOULD_BLOCK);
    CHECK(SetEvent(lock.release)); CHECK(WaitForSingleObject(worker, 2000) == WAIT_OBJECT_0);
    CHECK(CloseHandle(worker)); CHECK(CloseHandle(lock.ready)); CHECK(CloseHandle(lock.release));
    CALL(spt_sender_list_create(&list)); CALL(spt_sender_list_count(list, &count));
    CHECK(spt_sender_list_name(list, count, buffer, sizeof(buffer), &required) == SPT_INVALID_ARGUMENT);
    if (count != 0) {
        CHECK(spt_sender_list_name(list, 0, NULL, 0, &required) == SPT_BUFFER_TOO_SMALL);
        CHECK(required > 1);
    }
    CALL(spt_release(list));
    CHECK(spt_sender_list_count(list, &count) == SPT_INVALID_HANDLE);
    return receiver;
}

static void test_crop(void) {
    uint8_t alpha[64 * 64] = {0};
    spt_rect rect;
    spt_handle crop = 0;
    uint32_t content = 99;
    alpha[32 * 64 + 32] = 1;
    CALL(spt_visible_bounds(alpha, sizeof(alpha), 64, 64, &rect, &content));
    CHECK(content == 1 && rect.x == 24 && rect.y == 24 && rect.width == 17 && rect.height == 17);
    CHECK(spt_visible_bounds(NULL, sizeof(alpha), 64, 64, &rect, &content) == SPT_INVALID_ARGUMENT);
    CHECK(spt_visible_bounds(alpha, 1, 64, 64, &rect, &content) == SPT_INVALID_ARGUMENT);
    CHECK(spt_visible_bounds(alpha, SIZE_MAX, 64, 64, &rect, &content) == SPT_INVALID_ARGUMENT);
    CALL(spt_crop_create(64, 64, &crop));
    CALL(spt_crop_observe(crop, alpha, sizeof(alpha), 0, &rect)); CHECK(rect.width == 64);
    CALL(spt_crop_rect(crop, 499, &rect)); CHECK(rect.width == 64);
    CALL(spt_crop_rect(crop, 500, &rect)); CHECK(rect.width == 17 && rect.x == 24);
    CHECK(spt_crop_rect(crop, 499, &rect) == SPT_INVALID_ARGUMENT);
    alpha[0] = 255;
    CALL(spt_crop_observe(crop, alpha, sizeof(alpha), 501, &rect)); CHECK(rect.x == 0 && rect.width == 41);
    memset(alpha, 0, sizeof(alpha));
    CALL(spt_crop_observe(crop, alpha, sizeof(alpha), 502, &rect)); CHECK(rect.width == 41);
    CALL(spt_release(crop)); CHECK(spt_release(crop) == SPT_INVALID_HANDLE);
    CHECK(spt_crop_rect(UINT64_MAX, 0, &rect) == SPT_INVALID_HANDLE);
}

static void test_gpu(fixture *f, spt_handle receiver) {
    HWND window;
    spt_handle renderer = 99;
    uint32_t outcome = 0, width = 0, height = 0, connected = 0, pending = 99;
    size_t required = 0;
    uint8_t alpha[16 * 16];
    spt_status status;
    ULONGLONG deadline;
    spt_rect crop = {4, 3, 8, 9};
    spt_layout layout;
    RECT actual;
    CHECK(spt_renderer_create(NULL, &renderer) == SPT_INVALID_ARGUMENT && renderer == 0);
    window = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW, L"STATIC", L"C ABI test",
        WS_POPUP, -32000, -32000, 16, 16, NULL, NULL, NULL, NULL); CHECK(window != NULL);
    CALL(spt_renderer_create((void *)window, &renderer));
    CALL(spt_renderer_has_readback(renderer, &pending)); CHECK(pending == 0);
    CALL(spt_renderer_set_alpha(renderer, 1)); CALL(spt_renderer_attach(renderer, receiver));
    CHECK(spt_renderer_set_frame_rate(renderer, 61) == SPT_INVALID_ARGUMENT);
    CALL(spt_renderer_set_frame_rate(renderer, 0));
    CALL(spt_renderer_render(renderer, receiver, &outcome)); CHECK(outcome == SPT_FRAME_PRESENTED);
    CALL(spt_renderer_has_readback(renderer, &pending)); CHECK(pending == 1);
    deadline = GetTickCount64() + 3000;
    do {
        status = spt_renderer_read_alpha(renderer, NULL, 0, &required, &width, &height);
        if (status != SPT_WOULD_BLOCK) break;
        Sleep(1);
    } while (GetTickCount64() < deadline);
    CHECK(status == SPT_BUFFER_TOO_SMALL && required == sizeof(alpha) && width == 16 && height == 16);
    CALL(spt_renderer_has_readback(renderer, &pending)); CHECK(pending == 1);
    memset(alpha, 0xa5, sizeof(alpha));
    CHECK(spt_renderer_read_alpha(renderer, alpha, 1, &required, &width, &height) == SPT_BUFFER_TOO_SMALL);
    CHECK(alpha[0] == 0xa5);
    CALL(spt_renderer_read_alpha(renderer, alpha, sizeof(alpha), &required, &width, &height));
    { size_t i; for (i = 0; i < sizeof(alpha); ++i) CHECK(alpha[i] == 255); }
    CALL(spt_renderer_has_readback(renderer, &pending)); CHECK(pending == 0);
    CALL(spt_renderer_render(renderer, receiver, &outcome)); CHECK(outcome == SPT_FRAME_UNCHANGED);
    HR(IDXGIKeyedMutex_AcquireSync(f->keyed, 0, 0)); HR(IDXGIKeyedMutex_ReleaseSync(f->keyed, 7));
    CALL(spt_renderer_request_frame(renderer)); CALL(spt_renderer_render(renderer, receiver, &outcome));
    CHECK(outcome == SPT_FRAME_BUSY);
    HR(IDXGIKeyedMutex_AcquireSync(f->keyed, 7, 0)); HR(IDXGIKeyedMutex_ReleaseSync(f->keyed, 0));
    CALL(spt_renderer_request_frame(renderer)); CALL(spt_renderer_render(renderer, receiver, &outcome));
    CHECK(outcome == SPT_FRAME_PRESENTED);
    CALL(spt_renderer_set_layout(renderer, &crop, 1.5f)); CALL(spt_renderer_layout(renderer, &layout));
    CHECK(layout.crop.x == crop.x && layout.scale == 1.5f && layout.offset_x == 6 && layout.offset_y == 5);
    CHECK(GetWindowRect(window, &actual));
    CHECK(actual.left - layout.offset_x == -32000 && actual.top - layout.offset_y == -32000);
    CHECK(actual.right - actual.left == (LONG)layout.width && actual.bottom - actual.top == (LONG)layout.height);
    CHECK(spt_renderer_set_layout(renderer, &crop, 0.0f) == SPT_INVALID_ARGUMENT);
    CHECK(WaitForSingleObject(f->mutex, 2000) == WAIT_OBJECT_0);
    put_u32(f->metadata, 276, 2); CHECK(ReleaseMutex(f->mutex));
    Sleep(110); CALL(spt_receiver_poll(receiver, &connected)); CHECK(connected == 1);
    CHECK(spt_renderer_render(renderer, receiver, &outcome) == SPT_NOT_FOUND);
    CALL(spt_renderer_attach(renderer, receiver));
    CALL(spt_renderer_set_alpha(renderer, 0));
    CALL(spt_renderer_has_readback(renderer, &pending)); CHECK(pending == 0);
    CHECK(spt_renderer_read_alpha(renderer, alpha, sizeof(alpha), &required, &width, &height) == SPT_INVALID_ARGUMENT);
    CALL(spt_release(renderer)); CHECK(spt_release(renderer) == SPT_INVALID_HANDLE);
    CHECK(DestroyWindow(window));
}

int main(int argc, char **argv) {
    fixture f;
    spt_handle receiver;
    uint32_t connected = 1;
    int gpu = !(argc > 1 && strcmp(argv[1], "--no-gpu") == 0);
    HR(CoInitializeEx(NULL, COINIT_APARTMENTTHREADED));
    fixture_create(&f, gpu);
    receiver = test_metadata(&f); test_crop();
    if (gpu) test_gpu(&f, receiver);
    fixture_disconnect(&f); Sleep(110);
    CALL(spt_receiver_poll(receiver, &connected)); CHECK(connected == 0);
    CALL(spt_receiver_reset(receiver));
    CALL(spt_release(receiver)); CHECK(spt_release(receiver) == SPT_INVALID_HANDLE);
    fixture_destroy(&f);
    CoUninitialize();
#ifdef __cplusplus
    printf("C++ ABI checks passed (GPU=%s)\n", gpu ? "on" : "off");
#else
    printf("C ABI checks passed (GPU=%s)\n", gpu ? "on" : "off");
#endif
    return 0;
}
