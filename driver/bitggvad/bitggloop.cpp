/*++

Copyright (c) BitGG contributors

Module Name:

    bitggloop.cpp

Abstract:

    Mic loopback ring. See bitggloop.h.

--*/

#include <sysvad.h>
#include "bitggloop.h"

// ~340 ms of history at 192000 bytes/s. A power of two keeps the index math simple.
#define BITGG_LOOP_RING_BYTES       65536

// A reader that falls further behind than this skips ahead to keep latency low.
#define BITGG_LOOP_MAX_LAG_BYTES    (BITGG_LOOP_BYTES_PER_SEC / 10)    // 100 ms
#define BITGG_LOOP_RESYNC_LAG_BYTES (BITGG_LOOP_BYTES_PER_SEC / 50)    // 20 ms

C_ASSERT((BITGG_LOOP_RING_BYTES & (BITGG_LOOP_RING_BYTES - 1)) == 0);
C_ASSERT(BITGG_LOOP_MAX_LAG_BYTES < BITGG_LOOP_RING_BYTES);
C_ASSERT(BITGG_LOOP_MAX_LAG_BYTES % BITGG_LOOP_BLOCK_ALIGN == 0);
C_ASSERT(BITGG_LOOP_RESYNC_LAG_BYTES % BITGG_LOOP_BLOCK_ALIGN == 0);

static KSPIN_LOCK   g_LoopLock;
static ULONGLONG    g_LoopWritePos;     // total bytes ever written
static BYTE         g_LoopRing[BITGG_LOOP_RING_BYTES];

#pragma code_seg("INIT")
VOID BitggLoopInit()
{
    KeInitializeSpinLock(&g_LoopLock);
    g_LoopWritePos = 0;
    RtlZeroMemory(g_LoopRing, sizeof(g_LoopRing));
}

#pragma code_seg()
VOID BitggLoopWrite(const BYTE *Data, ULONG ByteCount)
{
    KIRQL irql;

    // Only the newest ring-full can ever be read.
    if (ByteCount > BITGG_LOOP_RING_BYTES)
    {
        Data += ByteCount - BITGG_LOOP_RING_BYTES;
        ByteCount = BITGG_LOOP_RING_BYTES;
    }

    KeAcquireSpinLock(&g_LoopLock, &irql);

    while (ByteCount > 0)
    {
        ULONG offset = (ULONG)(g_LoopWritePos & (BITGG_LOOP_RING_BYTES - 1));
        ULONG run = min(ByteCount, BITGG_LOOP_RING_BYTES - offset);
        RtlCopyMemory(g_LoopRing + offset, Data, run);
        g_LoopWritePos += run;
        Data += run;
        ByteCount -= run;
    }

    KeReleaseSpinLock(&g_LoopLock, irql);
}

#pragma code_seg()
ULONGLONG BitggLoopReaderStart()
{
    KIRQL irql;
    ULONGLONG pos;

    KeAcquireSpinLock(&g_LoopLock, &irql);
    pos = g_LoopWritePos;
    KeReleaseSpinLock(&g_LoopLock, irql);

    return pos;
}

#pragma code_seg()
VOID BitggLoopRead(ULONGLONG *ReadPos, BYTE *Data, ULONG ByteCount)
{
    KIRQL irql;

    KeAcquireSpinLock(&g_LoopLock, &irql);

    ULONGLONG readPos = *ReadPos;

    // Writer restarted from scratch (cannot happen today) or reader is ahead.
    if (readPos > g_LoopWritePos)
    {
        readPos = g_LoopWritePos;
    }

    // Too far behind: drop the backlog instead of growing the delay.
    if (g_LoopWritePos - readPos > BITGG_LOOP_MAX_LAG_BYTES)
    {
        readPos = g_LoopWritePos - BITGG_LOOP_RESYNC_LAG_BYTES;
    }

    ULONG available = (ULONG)(g_LoopWritePos - readPos);
    ULONG toCopy = min(available, ByteCount);

    // Keep whole frames so left/right never swap.
    toCopy -= toCopy % BITGG_LOOP_BLOCK_ALIGN;

    ULONG copied = 0;
    while (copied < toCopy)
    {
        ULONG offset = (ULONG)(readPos & (BITGG_LOOP_RING_BYTES - 1));
        ULONG run = min(toCopy - copied, BITGG_LOOP_RING_BYTES - offset);
        RtlCopyMemory(Data + copied, g_LoopRing + offset, run);
        readPos += run;
        copied += run;
    }

    *ReadPos = readPos;

    KeReleaseSpinLock(&g_LoopLock, irql);

    // Underrun: silence.
    if (copied < ByteCount)
    {
        RtlZeroMemory(Data + copied, ByteCount - copied);
    }
}
