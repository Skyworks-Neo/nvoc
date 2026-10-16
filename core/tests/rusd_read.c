/* CPU-only checks for the shared-page validity/freshness protocol. */
#include "rusd_read.h"
#include <assert.h>
#include <stdio.h>

int main(void)
{
    assert(!nvoc_rusd_fresh(0, 0));
    assert(!nvoc_rusd_fresh(RUSD_TIMESTAMP_WRITE_IN_PROGRESS, 0));
    assert(!nvoc_rusd_fresh(100, 100));
    assert(!nvoc_rusd_fresh(99, 100));
    assert(nvoc_rusd_fresh(101, 100));
    assert(nvoc_rusd_fresh(101, RUSD_TIMESTAMP_WRITE_IN_PROGRESS));
    assert(!nvoc_rusd_fresh(RUSD_SEQ_START + 1, 0));
    assert(nvoc_rusd_fresh(RUSD_SEQ_START + 2, RUSD_SEQ_START + 2));

    RUSD_TEMPERATURE field = { .lastModifiedTimestamp = 100, .temperature = -128 };
    RUSD_TEMPERATURE copy = {0};
    NvU64 observed = 0;
    assert(nvoc_rusd_read(&field.lastModifiedTimestamp, &field, &copy,
        sizeof copy, 99, &observed));
    assert(copy.temperature == -128 && observed == 100);
    assert(!nvoc_rusd_read(&field.lastModifiedTimestamp, &field, &copy,
        sizeof copy, 100, &observed));
    field.lastModifiedTimestamp = RUSD_TIMESTAMP_WRITE_IN_PROGRESS;
    assert(!nvoc_rusd_read(&field.lastModifiedTimestamp, &field, &copy,
        sizeof copy, 99, &observed));
    puts("RUSD validity, freshness and stable-copy checks passed");
}
