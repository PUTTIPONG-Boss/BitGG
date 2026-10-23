/*++

Copyright (c) Microsoft Corporation All Rights Reserved

Module Name:

    micintopotable.h

Abstract:

    Declaration of topology table for the mic (external: headphone)

--*/

#ifndef _SYSVAD_MICINTOPTABLE_H_
#define _SYSVAD_MICINTOPTABLE_H_

// Function declarations.
NTSTATUS
PropertyHandler_MicInTopoFilter( 
    _In_ PPCPROPERTY_REQUEST      PropertyRequest 
    );

// BitGG Mic endpoint name GUID. Keep in sync with bitggvad.inx [Strings].
static const GUID BitggNameMic = {0x496f0aa1, 0x3614, 0x4027, {0xb3, 0xbc, 0x28, 0xac, 0x2f, 0x3c, 0x66, 0xe0}};

//=============================================================================
// BitGG: virtual, always-connected stereo mic (no physical jack).
static
KSJACK_DESCRIPTION MicInJackDesc =
{
    KSAUDIO_SPEAKER_STEREO,
    0,
    eConnTypeUnknown,
    eGeoLocFront,
    eGenLocPrimaryBox,
    ePortConnIntegratedDevice,
    TRUE
};

//=============================================================================
static
KSDATARANGE MicInTopoPinDataRangesBridge[] =
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

//=============================================================================
static
PKSDATARANGE MicInTopoPinDataRangePointersBridge[] =
{
  &MicInTopoPinDataRangesBridge[0]
};

//=============================================================================
static
PCPIN_DESCRIPTOR MicInTopoMiniportPins[] =
{
  // KSPIN - topology filter in-pin
  {
    0,
    0,
    0,                                                  // InstanceCount
    NULL,                                               // AutomationTable
    {                                                   // KsPinDescriptor
      0,                                                // InterfacesCount
      NULL,                                             // Interfaces
      0,                                                // MediumsCount
      NULL,                                             // Mediums
      SIZEOF_ARRAY(MicInTopoPinDataRangePointersBridge),// DataRangesCount
      MicInTopoPinDataRangePointersBridge,              // DataRanges
      KSPIN_DATAFLOW_IN,                                // DataFlow
      KSPIN_COMMUNICATION_NONE,                         // Communication
      &KSNODETYPE_MICROPHONE,                           // Category
      &BitggNameMic,                                    // Name
      0                                                 // Reserved
    }
  },

  // KSPIN - topology filter out-in
  {
    0,
    0,
    0,                                                  // InstanceCount
    NULL,                                               // AutomationTable
    {                                                   // KsPinDescriptor
      0,                                                // InterfacesCount
      NULL,                                             // Interfaces
      0,                                                // MediumsCount
      NULL,                                             // Mediums
      SIZEOF_ARRAY(MicInTopoPinDataRangePointersBridge),// DataRangesCount
      MicInTopoPinDataRangePointersBridge,              // DataRanges
      KSPIN_DATAFLOW_OUT,                               // DataFlow
      KSPIN_COMMUNICATION_NONE,                         // Communication
      &KSCATEGORY_AUDIO,                                // Category
      NULL,                                             // Name
      0                                                 // Reserved
    }
  }
};

//=============================================================================
// Only return a KSJACK_DESCRIPTION for the physical bridge pin.
static 
PKSJACK_DESCRIPTION MicInJackDescriptions[] =
{
    &MicInJackDesc,
    NULL
};

//=============================================================================
// BitGG: no volume/mute/peak-meter nodes. The driver does no processing, so
// Windows applies endpoint volume and mute in software instead.
static
PCCONNECTION_DESCRIPTOR MicInMiniportConnections[] =
{
    //  FromNode,                 FromPin,                    ToNode,                 ToPin
    {   PCFILTER_NODE,            KSPIN_TOPO_MIC_ELEMENTS,    PCFILTER_NODE,          KSPIN_TOPO_BRIDGE }
};


//=============================================================================
static
PCPROPERTY_ITEM MicInPropertiesTopoFilter[] =
{
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicInTopoFilter
    },
    {
        &KSPROPSETID_Jack,
        KSPROPERTY_JACK_DESCRIPTION2,
        KSPROPERTY_TYPE_GET | KSPROPERTY_TYPE_BASICSUPPORT,
        PropertyHandler_MicInTopoFilter
    }
};

DEFINE_PCAUTOMATION_TABLE_PROP(AutomationMicInTopoFilter, MicInPropertiesTopoFilter);

//=============================================================================
static
PCFILTER_DESCRIPTOR MicInTopoMiniportFilterDescriptor =
{
  0,                                        // Version
  &AutomationMicInTopoFilter,               // AutomationTable
  sizeof(PCPIN_DESCRIPTOR),                 // PinSize
  SIZEOF_ARRAY(MicInTopoMiniportPins),      // PinCount
  MicInTopoMiniportPins,                    // Pins
  sizeof(PCNODE_DESCRIPTOR),                // NodeSize
  0,                                        // NodeCount
  NULL,                                     // Nodes
  SIZEOF_ARRAY(MicInMiniportConnections),   // ConnectionCount
  MicInMiniportConnections,                 // Connections
  0,                                        // CategoryCount
  NULL                                      // Categories
};

#endif // _SYSVAD_MICINTOPTABLE_H_

