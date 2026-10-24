/*++

Copyright (c) BitGG contributors

Module Name:

    bitggloop.h

Abstract:

    Mic loopback: audio rendered to "BitGG Mic (in)" is delivered to every
    stream capturing from "BitGG Mic" (docs/driver/SPEC.md section 4.3).

    Both sides use the same format (48 kHz, stereo, 16-bit), so this is a
    plain byte ring with no conversion. The writer appends; each capture
    stream keeps its own read position, so several apps can read at once.
    Everything runs at <= DISPATCH_LEVEL with no allocation.

--*/

#ifndef _BITGG_LOOP_H_
#define _BITGG_LOOP_H_

// 48 kHz * 2 ch * 2 bytes
#define BITGG_LOOP_BYTES_PER_SEC    192000
#define BITGG_LOOP_BLOCK_ALIGN      4

VOID BitggLoopInit();

// Render side: append ByteCount bytes.
VOID BitggLoopWrite(_In_reads_bytes_(ByteCount) const BYTE *Data, _In_ ULONG ByteCount);

// Capture side: a new reader starts at "now", so it never sees stale audio.
ULONGLONG BitggLoopReaderStart();

// Capture side: fill ByteCount bytes, padding with silence on underrun.
VOID BitggLoopRead(_Inout_ ULONGLONG *ReadPos, _Out_writes_bytes_(ByteCount) BYTE *Data, _In_ ULONG ByteCount);

#endif // _BITGG_LOOP_H_
