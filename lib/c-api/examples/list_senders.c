#include "spout_transparent.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failed(spt_status status) {
    if (status == SPT_OK) return 0;
    fprintf(stderr, "Spout error %d: %s\n", (int)status, spt_last_error_message());
    return 1;
}

int main(void) {
    spt_handle list = 0;
    size_t count = 0, index;
    int result = 1;
    if (spt_abi_version() != SPT_ABI_VERSION) {
        fprintf(stderr, "Unsupported Spout ABI version\n"); return 1;
    }
    if (failed(spt_sender_list_create(&list))) return 1;
    if (failed(spt_sender_list_count(list, &count))) goto done;
    printf("Spout senders: %zu\n", count);
    for (index = 0; index < count; ++index) {
        size_t required = 0;
        uint8_t *name;
        spt_sender_info info = {0};
        spt_status status = spt_sender_list_name(list, index, NULL, 0, &required);
        if (status != SPT_BUFFER_TOO_SMALL) { (void)failed(status); goto done; }
        name = (uint8_t *)malloc(required);
        if (name == NULL) goto done;
        if (failed(spt_sender_list_name(list, index, name, required, &required))) { free(name); goto done; }
        info.struct_size = (uint32_t)sizeof(info);
        status = spt_sender_info_read(name, strlen((const char *)name), 0, &info);
        if (status == SPT_OK) {
            printf("%s: %ux%u, DXGI %u\n", (const char *)name,
                (unsigned)info.width, (unsigned)info.height, (unsigned)info.format);
        } else {
            fprintf(stderr, "%s: %s\n", (const char *)name, spt_last_error_message());
        }
        free(name);
    }
    result = 0;
done:
    if (failed(spt_release(list))) result = 1;
    return result;
}
