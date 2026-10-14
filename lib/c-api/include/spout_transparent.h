#ifndef SPOUT_TRANSPARENT_H
#define SPOUT_TRANSPARENT_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
# define SPT_API __declspec(dllimport)
# define SPT_CALL __cdecl
#else
# define SPT_API
# define SPT_CALL
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define SPT_ABI_VERSION 1u
typedef uint64_t spt_handle;
typedef int32_t spt_status;
#define SPT_OK 0
#define SPT_INVALID_ARGUMENT 1
#define SPT_INVALID_HANDLE 2
#define SPT_BUFFER_TOO_SMALL 3
#define SPT_NOT_FOUND 4
#define SPT_WOULD_BLOCK 5
#define SPT_IO_ERROR 6
#define SPT_GPU_ERROR 7
#define SPT_PANIC 8

#define SPT_FRAME_PRESENTED 1u
#define SPT_FRAME_UNCHANGED 2u
#define SPT_FRAME_BUSY 3u
#define SPT_FRAME_NOT_DUE 4u
#define SPT_NO_DEADLINE UINT64_MAX

/* The ABI uses natural x64 alignment, independently of the includer's pack mode. */
#pragma pack(push, 8)
typedef struct spt_sender_info {
    uint32_t struct_size; /* Initialize to sizeof(spt_sender_info). */
    uint32_t width;
    uint32_t height;
    uint32_t format;      /* DXGI_FORMAT numeric code. */
    uint32_t usage;
    uint32_t partner_id;
    uint32_t share_handle; /* Spout wire value; sign-extend via int32_t/intptr_t for HANDLE. */
    uint32_t reserved;
    uint64_t generation; /* Zero for stateless read; receiver generations start at one. */
} spt_sender_info;

typedef struct spt_rect { uint32_t x, y, width, height; } spt_rect;
typedef struct spt_layout {
    spt_rect crop;
    float scale;
    int32_t offset_x, offset_y;
    uint32_t width, height;
} spt_layout;
#pragma pack(pop)

/* Contract (Windows x64, ABI 1):
 * - Handles are owned by their creating thread. Release them before that thread
 *   exits or the DLL unloads. Zero release is OK; stale, wrong-type and foreign-
 *   thread handles return INVALID_HANDLE. IDs are not pointers and are not reused
 *   within one DLL load.
 * - All pointers must address valid, suitably aligned memory for the documented
 *   lengths. Output buffers/fields must not overlap inputs or one another.
 *   NULL is accepted for buffer-size probes only with capacity=0. Pointer validity
 *   beyond null/alignment and accessible byte lengths remains the caller's duty.
 * - Raw sender names are byte strings: length EXCLUDES NUL, 1..255 bytes. Name
 *   outputs preserve native bytes and include a terminating NUL. Description and
 *   error text use UTF-8. Required name/description sizes INCLUDE the NUL.
 * - On BUFFER_TOO_SMALL, required is set and no partial buffer is copied. Alpha
 *   probes also set width/height and retain the sample until a successful copy.
 *   Other outputs are usable only on OK. Create functions zero a valid out handle
 *   before doing fallible work. No Rust allocation is handed to the caller.
 * - Ordinary calls clear last error on OK. Error/version queries preserve it.
 *   Error message is borrowed until the next ordinary call on the same thread.
 * - Recoverable Rust unwinds are contained as PANIC. Invalid memory access,
 *   allocation aborts, process aborts and panic=abort builds cannot be recovered.
 * - Do not invoke these functions from DllMain or loader-lock callbacks.
 */
SPT_API uint32_t SPT_CALL spt_abi_version(void);
SPT_API int32_t SPT_CALL spt_last_error_code(void);
SPT_API int32_t SPT_CALL spt_last_native_error(void);
SPT_API const char *SPT_CALL spt_last_error_message(void);
SPT_API spt_status SPT_CALL spt_release(spt_handle handle);

/* Synchronous discovery may wait for the registry lock (67 ms per wait).
 * Sender-list handles are immutable snapshots; they never register a sender. */
SPT_API spt_status SPT_CALL spt_sender_list_create(spt_handle *out);
SPT_API spt_status SPT_CALL spt_sender_list_count(spt_handle list, size_t *out);
SPT_API spt_status SPT_CALL spt_sender_list_name(spt_handle list, size_t index, uint8_t *buffer, size_t capacity, size_t *required);
SPT_API spt_status SPT_CALL spt_sender_info_read(const uint8_t *name, size_t length, uint32_t timeout_ms, spt_sender_info *info);

/* Refresh discovery separately (~500 ms); poll honors its own 100/500 ms metadata
 * deadlines and never waits on the sender mutex. Disconnected is OK/connected=0.
 * After reset, call refresh for automatic source selection. */
SPT_API spt_status SPT_CALL spt_receiver_create(spt_handle *out);
SPT_API spt_status SPT_CALL spt_receiver_refresh(spt_handle receiver);
SPT_API spt_status SPT_CALL spt_receiver_select(spt_handle receiver, const uint8_t *name, size_t length);
SPT_API spt_status SPT_CALL spt_receiver_reset(spt_handle receiver);
SPT_API spt_status SPT_CALL spt_receiver_poll(spt_handle receiver, uint32_t *connected);
SPT_API spt_status SPT_CALL spt_receiver_info(spt_handle receiver, spt_sender_info *info);
SPT_API spt_status SPT_CALL spt_receiver_name(spt_handle receiver, uint8_t *buffer, size_t capacity, size_t *required);
SPT_API spt_status SPT_CALL spt_receiver_description(spt_handle receiver, uint8_t *buffer, size_t capacity, size_t *required);
SPT_API spt_status SPT_CALL spt_receiver_next_poll_ms(spt_handle receiver, uint64_t *delay);

/* Alpha buffers are tightly packed width*height R8 bytes. Cropping detects any
 * nonzero alpha, pads 8 source pixels, grows immediately and shrinks after 500 ms.
 * Caller times are nondecreasing elapsed milliseconds, starting from zero per
 * crop handle; query rect after the delay without resubmitting the sample. */
SPT_API spt_status SPT_CALL spt_visible_bounds(const uint8_t *alpha, size_t length, uint32_t width, uint32_t height, spt_rect *rect, uint32_t *has_content);
SPT_API spt_status SPT_CALL spt_crop_create(uint32_t width, uint32_t height, spt_handle *out);
SPT_API spt_status SPT_CALL spt_crop_observe(spt_handle crop, const uint8_t *alpha, size_t length, uint64_t elapsed_ms, spt_rect *rect);
SPT_API spt_status SPT_CALL spt_crop_rect(spt_handle crop, uint64_t elapsed_ms, spt_rect *rect);

/* Host initializes COM before renderer_create and owns the HWND on this thread.
 * HWND must support DirectComposition (WS_EX_NOREDIRECTIONBITMAP, appropriate
 * transparency/input styles). Destroy renderer BEFORE HWND and CoUninitialize.
 * The DLL creates no host window, tray, hotkeys or global logger.
 * Attach after each receiver identity/generation change. Render refuses stale
 * resources. Present can wait for vsync; use deadlines, not a tight polling loop.
 * Alpha and automatic layout changes are off initially. Read alpha at host retry
 * deadlines (~2 ms), feed crop state, then explicitly set_layout. Extraction is
 * sampled at about 10 Hz; an explicit request_frame also forces a new attempt.
 */
SPT_API spt_status SPT_CALL spt_renderer_create(void *hwnd, spt_handle *out);
SPT_API spt_status SPT_CALL spt_renderer_attach(spt_handle renderer, spt_handle receiver);
SPT_API spt_status SPT_CALL spt_renderer_render(spt_handle renderer, spt_handle receiver, uint32_t *outcome);
SPT_API spt_status SPT_CALL spt_renderer_next_frame_ms(spt_handle renderer, uint64_t *delay);
SPT_API spt_status SPT_CALL spt_renderer_set_frame_rate(spt_handle renderer, uint32_t fps);
SPT_API spt_status SPT_CALL spt_renderer_request_frame(spt_handle renderer);
SPT_API spt_status SPT_CALL spt_renderer_set_alpha(spt_handle renderer, uint32_t enabled);
/* Pending includes a latched sample. Check after render; only retry alpha reads
 * at short intervals while this is 1, avoiding idle readback polling. */
SPT_API spt_status SPT_CALL spt_renderer_has_readback(spt_handle renderer, uint32_t *pending);
SPT_API spt_status SPT_CALL spt_renderer_read_alpha(spt_handle renderer, uint8_t *buffer, size_t capacity, size_t *required, uint32_t *width, uint32_t *height);
SPT_API spt_status SPT_CALL spt_renderer_set_layout(spt_handle renderer, const spt_rect *crop, float scale);
SPT_API spt_status SPT_CALL spt_renderer_layout(spt_handle renderer, spt_layout *layout);

#ifdef __cplusplus
}
#endif
#endif
