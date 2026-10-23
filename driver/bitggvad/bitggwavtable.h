/*++

Copyright (c) Microsoft Corporation All Rights Reserved
Portions copyright (c) BitGG contributors

Module Name:

    bitggwavtable.h

Abstract:

    Wave miniport tables for the BitGG endpoints. Derived from the SysVAD
    sample's render (no offload, no hardware loopback) and mic-in tables.

    Every pin runs at 48 kHz, 16-bit PCM. The Windows audio engine converts
    whatever apps send, so shared-mode apps can use any format.

    Render pin layout (simple.h):  KSPIN_WAVE_RENDER3_SINK_SYSTEM, KSPIN_WAVE_RENDER3_SOURCE
    Capture pin layout (simple.h): KSPIN_WAVE_BRIDGE, KSPIN_WAVEIN_HOST

--*/

#ifndef _BITGG_WAVTABLE_H_
#define _BITGG_WAVTABLE_H_

#define BITGG_SAMPLE_RATE               48000
#define BITGG_BITS_PER_SAMPLE           16

#define BITGG_STEREO_MAX_CHANNELS       2
#define BITGG_SURROUND_MAX_CHANNELS     8

#define BITGG_MAX_RENDER_STREAMS        6
#define BITGG_MAX_CAPTURE_STREAMS       6

#define BITGG_WAVEFORMAT(ch, mask)                                              \
    {                                                                           \
        {                                                                       \
            sizeof(KSDATAFORMAT_WAVEFORMATEXTENSIBLE),                          \
            0,                                                                  \
            0,                                                                  \
            0,                                                                  \
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),                              \
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),                             \
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)                   \
        },                                                                      \
        {                                                                       \
            {                                                                   \
                WAVE_FORMAT_EXTENSIBLE,                                         \
                (ch),                                                           \
                BITGG_SAMPLE_RATE,                                              \
                BITGG_SAMPLE_RATE * (ch) * (BITGG_BITS_PER_SAMPLE / 8),         \
                (ch) * (BITGG_BITS_PER_SAMPLE / 8),                             \
                BITGG_BITS_PER_SAMPLE,                                          \
                sizeof(WAVEFORMATEXTENSIBLE) - sizeof(WAVEFORMATEX)             \
            },                                                                  \
            BITGG_BITS_PER_SAMPLE,                                              \
            (mask),                                                             \
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM)                              \
        }                                                                       \
    }

//=============================================================================
// Device formats. The first entry is the default.
//=============================================================================
static
KSDATAFORMAT_WAVEFORMATEXTENSIBLE BitggStereoFormats[] =
{
    BITGG_WAVEFORMAT(2, KSAUDIO_SPEAKER_STEREO),
};

static
KSDATAFORMAT_WAVEFORMATEXTENSIBLE BitggSurroundFormats[] =
{
    BITGG_WAVEFORMAT(2, KSAUDIO_SPEAKER_STEREO),
    BITGG_WAVEFORMAT(8, KSAUDIO_SPEAKER_7POINT1_SURROUND),
};

static
MODE_AND_DEFAULT_FORMAT BitggStereoModes[] =
{
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
        &BitggStereoFormats[0].DataFormat
    },
};

static
MODE_AND_DEFAULT_FORMAT BitggSurroundModes[] =
{
    {
        STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
        &BitggSurroundFormats[0].DataFormat
    },
};

//
// The entries here must follow the same order as the filter's pin
// descriptor array.
//
static
PIN_DEVICE_FORMATS_AND_MODES BitggStereoRenderPinFormatsAndModes[] =
{
    {
        SystemRenderPin,
        BitggStereoFormats,
        SIZEOF_ARRAY(BitggStereoFormats),
        BitggStereoModes,
        SIZEOF_ARRAY(BitggStereoModes)
    },
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    }
};

static
PIN_DEVICE_FORMATS_AND_MODES BitggSurroundRenderPinFormatsAndModes[] =
{
    {
        SystemRenderPin,
        BitggSurroundFormats,
        SIZEOF_ARRAY(BitggSurroundFormats),
        BitggSurroundModes,
        SIZEOF_ARRAY(BitggSurroundModes)
    },
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    }
};

static
PIN_DEVICE_FORMATS_AND_MODES BitggCapturePinFormatsAndModes[] =
{
    {
        BridgePin,
        NULL,
        0,
        NULL,
        0
    },
    {
        SystemCapturePin,
        BitggStereoFormats,
        SIZEOF_ARRAY(BitggStereoFormats),
        BitggStereoModes,
        SIZEOF_ARRAY(BitggStereoModes)
    }
};

//=============================================================================
// Data ranges
//=============================================================================
static
KSDATARANGE_AUDIO BitggStereoDataRanges[] =
{
    {
        {
            sizeof(KSDATARANGE_AUDIO),
            KSDATARANGE_ATTRIBUTES,         // An attributes list follows this data range
            0,
            0,
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        },
        BITGG_STEREO_MAX_CHANNELS,
        BITGG_BITS_PER_SAMPLE,
        BITGG_BITS_PER_SAMPLE,
        BITGG_SAMPLE_RATE,
        BITGG_SAMPLE_RATE
    },
};

static
PKSDATARANGE BitggStereoDataRangePointers[] =
{
    PKSDATARANGE(&BitggStereoDataRanges[0]),
    PKSDATARANGE(&PinDataRangeAttributeList),
};

static
KSDATARANGE_AUDIO BitggSurroundDataRanges[] =
{
    {
        {
            sizeof(KSDATARANGE_AUDIO),
            KSDATARANGE_ATTRIBUTES,         // An attributes list follows this data range
            0,
            0,
            STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
            STATICGUIDOF(KSDATAFORMAT_SUBTYPE_PCM),
            STATICGUIDOF(KSDATAFORMAT_SPECIFIER_WAVEFORMATEX)
        },
        BITGG_SURROUND_MAX_CHANNELS,
        BITGG_BITS_PER_SAMPLE,
        BITGG_BITS_PER_SAMPLE,
        BITGG_SAMPLE_RATE,
        BITGG_SAMPLE_RATE
    },
};

static
PKSDATARANGE BitggSurroundDataRangePointers[] =
{
    PKSDATARANGE(&BitggSurroundDataRanges[0]),
    PKSDATARANGE(&PinDataRangeAttributeList),
};

static
KSDATARANGE BitggBridgeDataRanges[] =
{
    {
        sizeof(KSDATARANGE),
        0,
        0,
        0,
        STATICGUIDOF(KSDATAFORMAT_TYPE_AUDIO),
        STATICGUIDOF(KSDATAFORMAT_SUBTYPE_ANALOG),
        STATICGUIDOF(KSDATAFORMAT_SPECIFIER_NONE)
    }
};

static
PKSDATARANGE BitggBridgeDataRangePointers[] =
{
    &BitggBridgeDataRanges[0]
};

//=============================================================================
// Pins
//=============================================================================
#define BITGG_RENDER_PINS(name, ranges)                                         \
static                                                                          \
PCPIN_DESCRIPTOR name[] =                                                       \
{                                                                               \
    /* KSPIN_WAVE_RENDER3_SINK_SYSTEM */                                        \
    {                                                                           \
        BITGG_MAX_RENDER_STREAMS,                                               \
        BITGG_MAX_RENDER_STREAMS,                                               \
        0,                                                                      \
        NULL,                                                                   \
        {                                                                       \
            0,                                                                  \
            NULL,                                                               \
            0,                                                                  \
            NULL,                                                               \
            SIZEOF_ARRAY(ranges),                                               \
            ranges,                                                             \
            KSPIN_DATAFLOW_IN,                                                  \
            KSPIN_COMMUNICATION_SINK,                                           \
            &KSCATEGORY_AUDIO,                                                  \
            NULL,                                                               \
            0                                                                   \
        }                                                                       \
    },                                                                          \
    /* KSPIN_WAVE_RENDER3_SOURCE */                                             \
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
            SIZEOF_ARRAY(BitggBridgeDataRangePointers),                         \
            BitggBridgeDataRangePointers,                                       \
            KSPIN_DATAFLOW_OUT,                                                 \
            KSPIN_COMMUNICATION_NONE,                                           \
            &KSCATEGORY_AUDIO,                                                  \
            NULL,                                                               \
            0                                                                   \
        }                                                                       \
    },                                                                          \
};

BITGG_RENDER_PINS(BitggStereoRenderPins, BitggStereoDataRangePointers)
BITGG_RENDER_PINS(BitggSurroundRenderPins, BitggSurroundDataRangePointers)

static
PCPIN_DESCRIPTOR BitggCapturePins[] =
{
    // KSPIN_WAVE_BRIDGE
    {
        0,
        0,
        0,
        NULL,
        {
            0,
            NULL,
            0,
            NULL,
            SIZEOF_ARRAY(BitggBridgeDataRangePointers),
            BitggBridgeDataRangePointers,
            KSPIN_DATAFLOW_IN,
            KSPIN_COMMUNICATION_NONE,
            &KSCATEGORY_AUDIO,
            NULL,
            0
        }
    },
    // KSPIN_WAVEIN_HOST
    {
        BITGG_MAX_CAPTURE_STREAMS,
        BITGG_MAX_CAPTURE_STREAMS,
        0,
        NULL,
        {
            0,
            NULL,
            0,
            NULL,
            SIZEOF_ARRAY(BitggStereoDataRangePointers),
            BitggStereoDataRangePointers,
            KSPIN_DATAFLOW_OUT,
            KSPIN_COMMUNICATION_SINK,
            &KSCATEGORY_AUDIO,
            &KSAUDFNAME_RECORDING_CONTROL,
            0
        }
    }
};

//=============================================================================
// Nodes and connections
//=============================================================================
static
PCNODE_DESCRIPTOR BitggRenderNodes[] =
{
    // KSNODE_WAVE_DAC
    {
        0,                      // Flags
        NULL,                   // AutomationTable
        &KSNODETYPE_DAC,        // Type
        NULL                    // Name
    }
};

static
PCCONNECTION_DESCRIPTOR BitggRenderConnections[] =
{
    { PCFILTER_NODE,    KSPIN_WAVE_RENDER3_SINK_SYSTEM, KSNODE_WAVE_DAC,    1 },
    { KSNODE_WAVE_DAC,  0,                              PCFILTER_NODE,      KSPIN_WAVE_RENDER3_SOURCE },
};

static
PCNODE_DESCRIPTOR BitggCaptureNodes[] =
{
    // KSNODE_WAVE_ADC
    {
        0,                      // Flags
        NULL,                   // AutomationTable
        &KSNODETYPE_ADC,        // Type
        NULL                    // Name
    }
};

static
PCCONNECTION_DESCRIPTOR BitggCaptureConnections[] =
{
    { PCFILTER_NODE,    KSPIN_WAVE_BRIDGE,  KSNODE_WAVE_ADC,    1 },
    { KSNODE_WAVE_ADC,  0,                  PCFILTER_NODE,      KSPIN_WAVEIN_HOST },
};

//=============================================================================
// Filter properties and descriptors
//=============================================================================
static
PCPROPERTY_ITEM BitggWaveFilterProperties[] =
{
    {
        &KSPROPSETID_General,
        KSPROPERTY_GENERAL_COMPONENTID,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    },
    {
        &KSPROPSETID_Pin,
        KSPROPERTY_PIN_PROPOSEDATAFORMAT,
        KSPROPERTY_TYPE_SET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    },
    {
        &KSPROPSETID_Pin,
        KSPROPERTY_PIN_PROPOSEDATAFORMAT2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_WaveFilter
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationBitggWaveFilter, BitggWaveFilterProperties);

#define BITGG_FILTER(name, pins, nodes, conns)                                  \
static                                                                          \
PCFILTER_DESCRIPTOR name =                                                      \
{                                                                               \
    0,                                  /* Version         */                   \
    &AutomationBitggWaveFilter,         /* AutomationTable */                   \
    sizeof(PCPIN_DESCRIPTOR),           /* PinSize         */                   \
    SIZEOF_ARRAY(pins),                 /* PinCount        */                   \
    pins,                               /* Pins            */                   \
    sizeof(PCNODE_DESCRIPTOR),          /* NodeSize        */                   \
    SIZEOF_ARRAY(nodes),                /* NodeCount       */                   \
    nodes,                              /* Nodes           */                   \
    SIZEOF_ARRAY(conns),                /* ConnectionCount */                   \
    conns,                              /* Connections     */                   \
    0,                                  /* CategoryCount   */                   \
    NULL                                /* Categories      */                   \
};

BITGG_FILTER(BitggStereoRenderWaveFilter,   BitggStereoRenderPins,   BitggRenderNodes,  BitggRenderConnections)
BITGG_FILTER(BitggSurroundRenderWaveFilter, BitggSurroundRenderPins, BitggRenderNodes,  BitggRenderConnections)
BITGG_FILTER(BitggCaptureWaveFilter,        BitggCapturePins,        BitggCaptureNodes, BitggCaptureConnections)

#endif // _BITGG_WAVTABLE_H_
