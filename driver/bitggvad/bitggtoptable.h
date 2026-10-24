/*++

Copyright (c) Microsoft Corporation All Rights Reserved
Portions copyright (c) BitGG contributors

Module Name:

    bitggtoptable.h

Abstract:

    Topology filters for the BitGG render endpoints. Same shape as SysVAD's
    speaker topology (no nodes); each endpoint gets its own filter only so
    its line-out pin can carry a name GUID. Windows shows that name, from the
    INF's MediaCategories entry, as the endpoint name ("BitGG Game", ...).

    Must be included after speakertoptable.h.

--*/

#ifndef _BITGG_TOPTABLE_H_
#define _BITGG_TOPTABLE_H_

// Endpoint name GUIDs. Keep in sync with bitggvad.inx [Strings].
static const GUID BitggNameGame  = {0x95663dfc, 0x3d84, 0x4530, {0xb8, 0xcb, 0xc3, 0xa1, 0xe3, 0x40, 0x8f, 0x36}};
static const GUID BitggNameChat  = {0x62c8d2d0, 0xa743, 0x4bd9, {0x91, 0xea, 0x9d, 0xda, 0x98, 0x55, 0x54, 0xb8}};
static const GUID BitggNameMedia = {0x57732222, 0x9ac1, 0x4675, {0xb0, 0x0f, 0x83, 0x53, 0x28, 0x2a, 0xd9, 0xfc}};
static const GUID BitggNameAux   = {0x59e39824, 0xe65b, 0x4638, {0xbf, 0xe5, 0x09, 0x47, 0xc0, 0xd7, 0xf0, 0x66}};
static const GUID BitggNameMicIn = {0x7dca001f, 0xcdb5, 0x404e, {0x95, 0x6a, 0x28, 0xdc, 0x49, 0x6a, 0xef, 0xc0}};

#define BITGG_RENDER_TOPO(filter, pins, name)                                   \
static                                                                          \
PCPIN_DESCRIPTOR pins[] =                                                       \
{                                                                               \
    /* KSPIN_TOPO_WAVEOUT_SOURCE */                                             \
    {                                                                           \
        0,                                                                      \
        0,                                                                      \
        0,                                                                      \
        NULL,                                                                   \
        {                                                                       \
            0,                                                                  \
            NULL,                                                               \
            0,                                                                  \
            NULL,                                                               \
            SIZEOF_ARRAY(SpeakerTopoPinDataRangePointersBridge),                \
            SpeakerTopoPinDataRangePointersBridge,                              \
            KSPIN_DATAFLOW_IN,                                                  \
            KSPIN_COMMUNICATION_NONE,                                           \
            &KSCATEGORY_AUDIO,                                                  \
            NULL,                                                               \
            0                                                                   \
        }                                                                       \
    },                                                                          \
    /* KSPIN_TOPO_LINEOUT_DEST */                                               \
    {                                                                           \
        0,                                                                      \
        0,                                                                      \
        0,                                                                      \
        NULL,                                                                   \
        {                                                                       \
            0,                                                                  \
            NULL,                                                               \
            0,                                                                  \
            NULL,                                                               \
            SIZEOF_ARRAY(SpeakerTopoPinDataRangePointersBridge),                \
            SpeakerTopoPinDataRangePointersBridge,                              \
            KSPIN_DATAFLOW_OUT,                                                 \
            KSPIN_COMMUNICATION_NONE,                                           \
            &KSNODETYPE_SPEAKER,                                                \
            &name,                                                              \
            0                                                                   \
        }                                                                       \
    }                                                                           \
};                                                                              \
                                                                                \
static                                                                          \
PCFILTER_DESCRIPTOR filter =                                                    \
{                                                                               \
    0,                                              /* Version         */       \
    &AutomationSpeakerTopoFilter,                   /* AutomationTable */       \
    sizeof(PCPIN_DESCRIPTOR),                       /* PinSize         */       \
    SIZEOF_ARRAY(pins),                             /* PinCount        */       \
    pins,                                           /* Pins            */       \
    sizeof(PCNODE_DESCRIPTOR),                      /* NodeSize        */       \
    0,                                              /* NodeCount       */       \
    NULL,                                           /* Nodes           */       \
    SIZEOF_ARRAY(SpeakerTopoMiniportConnections),   /* ConnectionCount */       \
    SpeakerTopoMiniportConnections,                 /* Connections     */       \
    0,                                              /* CategoryCount   */       \
    NULL                                            /* Categories      */       \
};

BITGG_RENDER_TOPO(BitggGameTopoFilter,  BitggGameTopoPins,  BitggNameGame)
BITGG_RENDER_TOPO(BitggChatTopoFilter,  BitggChatTopoPins,  BitggNameChat)
BITGG_RENDER_TOPO(BitggMediaTopoFilter, BitggMediaTopoPins, BitggNameMedia)
BITGG_RENDER_TOPO(BitggAuxTopoFilter,   BitggAuxTopoPins,   BitggNameAux)
BITGG_RENDER_TOPO(BitggMicInTopoFilter, BitggMicInTopoPins, BitggNameMicIn)

#endif // _BITGG_TOPTABLE_H_
