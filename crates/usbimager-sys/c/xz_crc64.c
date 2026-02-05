#include <stdint.h>
#include <stddef.h>
#include "xz.h"

#ifdef XZ_USE_CRC64

static uint64_t crc64_table[256];
static int crc64_ready = 0;

void xz_crc64_init(void)
{
    if(crc64_ready) return;
    for(unsigned int i = 0; i < 256; i++) {
        uint64_t r = (uint64_t)i;
        for(unsigned int j = 0; j < 8; j++) {
            if(r & 1)
                r = (r >> 1) ^ 0xC96C5795D7870F42ULL;
            else
                r >>= 1;
        }
        crc64_table[i] = r;
    }
    crc64_ready = 1;
}

uint64_t xz_crc64(const uint8_t *buf, size_t size, uint64_t crc)
{
    crc = ~crc;
    while(size--) {
        crc = crc64_table[(crc ^ *buf++) & 0xFF] ^ (crc >> 8);
    }
    return ~crc;
}

#endif
