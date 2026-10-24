/*++

Copyright (c) Microsoft Corporation All Rights Reserved
Portions copyright (c) BitGG contributors

Module Name:

    minipairs.h

Abstract:

    BitGG endpoint filter definitions (see docs/driver/SPEC.md section 4.2).

    Every endpoint is a topology + wave miniport pair. The names here must
    match the KSNAME_* entries in bitggvad.inx.

--*/

#ifndef _BITGG_MINIPAIRS_H_
#define _BITGG_MINIPAIRS_H_

#include "speakertopo.h"
#include "speakertoptable.h"
#include "bitggtoptable.h"

#include "micintopo.h"
#include "micintoptable.h"

#include "bitggwavtable.h"

NTSTATUS
CreateMiniportWaveRTSYSVAD
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

NTSTATUS
CreateMiniportTopologySYSVAD
(
    _Out_       PUNKNOWN *,
    _In_        REFCLSID,
    _In_opt_    PUNKNOWN,
    _In_        POOL_FLAGS,
    _In_        PUNKNOWN,
    _In_opt_    PVOID,
    _In_        PENDPOINT_MINIPAIR
);

/*********************************************************************
* Render endpoints (Game, Chat, Media, Aux, Mic (in))                *
*                                                                    *
*              +------+                +------+                      *
*              | Wave |                | Topo |                      *
*              |      |                |      |                      *
* System   --->|0    1|--------------->|0    1|---> Line Out         *
*              |      |                |      |                      *
*              +------+                +------+                      *
*                                                                    *
* No hardware loopback pin: the Windows audio engine provides        *
* WASAPI loopback for these endpoints, which is how BitGG reads them.*
*********************************************************************/
static
PHYSICALCONNECTIONTABLE BitggRenderPhysicalConnections[] =
{
    {
        KSPIN_TOPO_WAVEOUT_SOURCE,  // TopologyIn
        KSPIN_WAVE_RENDER3_SOURCE,  // WaveOut
        CONNECTIONTYPE_WAVE_OUTPUT
    }
};

#define BITGG_RENDER_MINIPAIR(var, type, topo, topoDesc, wave, waveDesc, maxCh, formats) \
static                                                                          \
ENDPOINT_MINIPAIR var =                                                         \
{                                                                               \
    type,                                                                       \
    topo,                                                                       \
    NULL,                                                                       \
    CreateMiniportTopologySYSVAD,                                               \
    &topoDesc,                                                                  \
    0, NULL,                                                                    \
    wave,                                                                       \
    NULL,                                                                       \
    CreateMiniportWaveRTSYSVAD,                                                 \
    &waveDesc,                                                                  \
    0, NULL,                                                                    \
    maxCh,                                                                      \
    formats,                                                                    \
    SIZEOF_ARRAY(formats),                                                      \
    BitggRenderPhysicalConnections,                                             \
    SIZEOF_ARRAY(BitggRenderPhysicalConnections),                               \
    ENDPOINT_NO_FLAGS,                                                          \
    NULL, 0, NULL,                                                              \
};

BITGG_RENDER_MINIPAIR(GameMiniports,  eBitggGameDevice,  L"TopologyGame",  BitggGameTopoFilter,  L"WaveGame",
                      BitggSurroundRenderWaveFilter, BITGG_SURROUND_MAX_CHANNELS, BitggSurroundRenderPinFormatsAndModes)
BITGG_RENDER_MINIPAIR(ChatMiniports,  eBitggChatDevice,  L"TopologyChat",  BitggChatTopoFilter,  L"WaveChat",
                      BitggStereoRenderWaveFilter, BITGG_STEREO_MAX_CHANNELS, BitggStereoRenderPinFormatsAndModes)
BITGG_RENDER_MINIPAIR(MediaMiniports, eBitggMediaDevice, L"TopologyMedia", BitggMediaTopoFilter, L"WaveMedia",
                      BitggStereoRenderWaveFilter, BITGG_STEREO_MAX_CHANNELS, BitggStereoRenderPinFormatsAndModes)
BITGG_RENDER_MINIPAIR(AuxMiniports,   eBitggAuxDevice,   L"TopologyAux",   BitggAuxTopoFilter,   L"WaveAux",
                      BitggStereoRenderWaveFilter, BITGG_STEREO_MAX_CHANNELS, BitggStereoRenderPinFormatsAndModes)
BITGG_RENDER_MINIPAIR(MicInMiniports, eBitggMicInDevice, L"TopologyMicIn", BitggMicInTopoFilter, L"WaveMicIn",
                      BitggStereoRenderWaveFilter, BITGG_STEREO_MAX_CHANNELS, BitggStereoRenderPinFormatsAndModes)

/*********************************************************************
* Capture endpoint (Mic). Reads what was rendered to Mic (in).       *
*                                                                    *
*              +------+    +------+                                  *
*              | Topo |    | Wave |                                  *
*              |      |    |      |                                  *
*  Mic in  --->|0    1|===>|0    1|---> Capture Host Pin             *
*              |      |    |      |                                  *
*              +------+    +------+                                  *
*********************************************************************/
static
PHYSICALCONNECTIONTABLE BitggMicPhysicalConnections[] =
{
    {
        KSPIN_TOPO_BRIDGE,          // TopologyOut
        KSPIN_WAVE_BRIDGE,          // WaveIn
        CONNECTIONTYPE_TOPOLOGY_OUTPUT
    }
};

static
ENDPOINT_MINIPAIR MicMiniports =
{
    eBitggMicDevice,
    L"TopologyMic",
    NULL,
    CreateMiniportTopologySYSVAD,
    &MicInTopoMiniportFilterDescriptor,
    0, NULL,
    L"WaveMic",
    NULL,
    CreateMiniportWaveRTSYSVAD,
    &BitggCaptureWaveFilter,
    0, NULL,
    BITGG_STEREO_MAX_CHANNELS,
    BitggCapturePinFormatsAndModes,
    SIZEOF_ARRAY(BitggCapturePinFormatsAndModes),
    BitggMicPhysicalConnections,
    SIZEOF_ARRAY(BitggMicPhysicalConnections),
    ENDPOINT_NO_FLAGS,
    NULL, 0, NULL,
};

//=============================================================================
static
PENDPOINT_MINIPAIR  g_RenderEndpoints[] =
{
    &GameMiniports,
    &ChatMiniports,
    &MediaMiniports,
    &AuxMiniports,
    &MicInMiniports,
};

#define g_cRenderEndpoints  (SIZEOF_ARRAY(g_RenderEndpoints))

static
PENDPOINT_MINIPAIR  g_CaptureEndpoints[] =
{
    &MicMiniports,
};

#define g_cCaptureEndpoints (SIZEOF_ARRAY(g_CaptureEndpoints))

//
// Total miniports = # endpoints * 2 (topology + wave).
//
#define g_MaxMiniports  ((g_cRenderEndpoints + g_cCaptureEndpoints) * 2)

#endif // _BITGG_MINIPAIRS_H_
