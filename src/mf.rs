//! Native Media Foundation encode + mux (no ffmpeg).
//!
//! Moments' gsdk keeps encoded packets in a "clip buffer" and muxes on demand
//! (ffmpeg_muxer). We do the same with OS components only:
//!   NV12 D3D11 texture -> hardware H.264 encoder MFT (NVENC/QSV/AMF, async)
//!   PCM s16 -> AAC encoder MFT
//!   compressed packets -> MP4 via IMFSinkWriter in pass-through (no re-encode)

#![allow(non_upper_case_globals)]

use std::mem::ManuallyDrop;
use std::sync::{Arc, Once};
use windows::core::{Interface, Result, GUID, VARIANT};
use windows::Win32::Foundation::{E_FAIL, TRUE};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

const CODECAPI_GOP: GUID = GUID::from_u128(0x95f31b26_95a4_41aa_9303_246a7fc6eef1);
const CODECAPI_FORCE_KEY: GUID = GUID::from_u128(0x398c1b98_8353_475a_9ef2_8f265d260345);
const CODECAPI_RC_MODE: GUID = GUID::from_u128(0x1c0608e9_370c_4710_8a58_cb6181c42423);
const CODECAPI_MEAN_BR: GUID = GUID::from_u128(0xf7222374_2144_4815_b550_a37f8e12ee52);
const CODECAPI_LOW_LATENCY: GUID = GUID::from_u128(0x9c27891a_ed7a_40e1_88e8_b22727a024ee);

pub fn startup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let _ = MFStartup(0x0002_0070, MFSTARTUP_FULL);
    });
}

/// One compressed packet. `data` is shared so snapshots of the ring are cheap.
#[derive(Clone)]
pub struct Packet {
    pub data: Arc<[u8]>,
    pub pts: i64, // 100 ns
    pub dur: i64,
    pub key: bool,
}

fn pack(w: u32, h: u32) -> u64 {
    ((w as u64) << 32) | h as u64
}

unsafe fn sample_bytes(s: &IMFSample) -> Result<Vec<u8>> {
    let buf = s.ConvertToContiguousBuffer()?;
    let mut p = std::ptr::null_mut();
    let mut len = 0u32;
    buf.Lock(&mut p, None, Some(&mut len))?;
    let v = std::slice::from_raw_parts(p, len as usize).to_vec();
    buf.Unlock()?;
    Ok(v)
}

pub unsafe fn mem_sample(data: &[u8], pts: i64, dur: i64, key: bool) -> Result<IMFSample> {
    let buf = MFCreateMemoryBuffer(data.len() as u32)?;
    let mut p = std::ptr::null_mut();
    buf.Lock(&mut p, None, None)?;
    std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
    buf.Unlock()?;
    buf.SetCurrentLength(data.len() as u32)?;
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    s.SetSampleTime(pts)?;
    s.SetSampleDuration(dur)?;
    if key {
        s.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
    }
    Ok(s)
}

fn enum_mfts(cat: GUID, flags: MFT_ENUM_FLAG, inp: GUID, maj: GUID, out: GUID) -> Vec<IMFActivate> {
    let mut v = Vec::new();
    unsafe {
        let i = MFT_REGISTER_TYPE_INFO { guidMajorType: maj, guidSubtype: inp };
        let o = MFT_REGISTER_TYPE_INFO { guidMajorType: maj, guidSubtype: out };
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        if MFTEnumEx(cat, flags, Some(&i), Some(&o), &mut arr, &mut n).is_ok() && !arr.is_null() {
            for k in 0..n as usize {
                if let Some(a) = (*arr.add(k)).take() {
                    v.push(a);
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
    }
    v
}

fn friendly(a: &IMFActivate) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut len = 0u32;
        if a.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len)).is_ok() {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
    }
    "MFT".into()
}

/// Extract SPS+PPS (Annex-B, with start codes) from an IDR access unit.
fn sps_pps(au: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    let n = au.len();
    let mut starts = Vec::new();
    while i + 3 < n {
        if au[i] == 0 && au[i + 1] == 0 && (au[i + 2] == 1 || (au[i + 2] == 0 && au[i + 3] == 1)) {
            let sc = if au[i + 2] == 1 { 3 } else { 4 };
            starts.push((i, i + sc));
            i += sc;
        } else {
            i += 1;
        }
    }
    for (k, &(_, body)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|s| s.0).unwrap_or(n);
        if body < end {
            let t = au[body] & 0x1f;
            if t == 7 || t == 8 {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&au[body..end]);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ===================================================================== video

pub struct VideoEnc {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    codec: Option<ICodecAPI>,
    need_input: u32,
    provides_samples: bool,
    out_size: u32,
    pub name: String,
    pub hw_textures: bool,
    /// SPS+PPS (Annex-B) once the first IDR came out.
    pub hdr: Option<Vec<u8>>,
    w: u32,
    h: u32,
    fps: u32,
    bitrate: u32,
}

impl VideoEnc {
    pub fn new(device: &ID3D11Device, w: u32, h: u32, fps: u32, bitrate: u32, prefer: &str) -> Result<VideoEnc> {
        startup();
        let mut cands = enum_mfts(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            MFVideoFormat_NV12,
            MFMediaType_Video,
            MFVideoFormat_H264,
        );
        // prefer the vendor the user picked (nvenc -> NVIDIA, qsv -> Intel, amf -> AMD)
        let want = match prefer {
            p if p.contains("nvenc") => "nvidia",
            p if p.contains("qsv") => "intel",
            p if p.contains("amf") => "amd",
            _ => "",
        };
        if !want.is_empty() {
            cands.sort_by_key(|a| !friendly(a).to_lowercase().contains(want));
        }
        if prefer == "software" {
            cands.clear();
        }
        cands.extend(enum_mfts(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            MFVideoFormat_NV12,
            MFMediaType_Video,
            MFVideoFormat_H264,
        ));
        let mut last = windows::core::Error::from(E_FAIL);
        for a in cands {
            match Self::try_one(&a, device, w, h, fps, bitrate) {
                Ok(e) => return Ok(e),
                Err(e) => last = e,
            }
            unsafe {
                let _ = a.ShutdownObject();
            }
        }
        Err(last)
    }

    fn try_one(a: &IMFActivate, device: &ID3D11Device, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<VideoEnc> {
        unsafe {
            let name = friendly(a);
            let mft: IMFTransform = a.ActivateObject()?;
            let attrs = mft.GetAttributes()?;
            let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
            if is_async {
                attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            // hand the encoder our D3D11 device so it can read GPU textures directly
            let mut hw_textures = false;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0 {
                if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                    let _ = mt.SetMultithreadProtected(TRUE);
                }
                let mut token = 0u32;
                let mut mgr: Option<IMFDXGIDeviceManager> = None;
                MFCreateDXGIDeviceManager(&mut token, &mut mgr)?;
                let mgr = mgr.unwrap();
                mgr.ResetDevice(device, token)?;
                hw_textures = mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, mgr.as_raw() as usize).is_ok();
                std::mem::forget(mgr); // the MFT holds a reference; keep ours for its lifetime
            }
            let codec = mft.cast::<ICodecAPI>().ok();
            if let Some(c) = &codec {
                let _ = c.SetValue(&CODECAPI_RC_MODE, &VARIANT::from(0u32)); // CBR
                let _ = c.SetValue(&CODECAPI_MEAN_BR, &VARIANT::from(bitrate));
                let _ = c.SetValue(&CODECAPI_GOP, &VARIANT::from(fps * 2));
                let _ = c.SetValue(&CODECAPI_LOW_LATENCY, &VARIANT::from(true));
            }

            let ot = MFCreateMediaType()?;
            ot.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            ot.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            ot.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
            ot.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            ot.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            ot.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            ot.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            ot.SetUINT32(&MF_MT_MPEG2_PROFILE, 100)?; // High
            ot.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, fps * 2)?;
            mft.SetOutputType(0, &ot, 0)?;

            let it = MFCreateMediaType()?;
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            it.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            it.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            it.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            it.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            it.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            it.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            it.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            mft.SetInputType(0, &it, 0)?;

            let info = mft.GetOutputStreamInfo(0)?;
            let provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let events = if is_async { Some(mft.cast::<IMFMediaEventGenerator>()?) } else { None };
            Ok(VideoEnc {
                mft,
                events,
                codec,
                need_input: 0,
                provides_samples,
                out_size: info.cbSize.max(w * h * 2),
                name,
                hw_textures,
                hdr: None,
                w,
                h,
                fps,
                bitrate,
            })
        }
    }

    pub fn force_keyframe(&self) {
        if let Some(c) = &self.codec {
            unsafe {
                let _ = c.SetValue(&CODECAPI_FORCE_KEY, &VARIANT::from(1u32));
            }
        }
    }

    /// Build the sample for one NV12 frame: a D3D11 surface when the encoder
    /// is D3D-aware, otherwise CPU bytes supplied by `cpu`.
    pub unsafe fn texture_sample(tex: &ID3D11Texture2D, pts: i64, dur: i64) -> Result<IMFSample> {
        let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false)?;
        if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
            let len = b2.GetContiguousLength()?;
            buf.SetCurrentLength(len)?;
        }
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(pts)?;
        s.SetSampleDuration(dur)?;
        Ok(s)
    }

    /// Feed one frame, collect whatever packets are ready.
    pub fn encode(&mut self, sample: &IMFSample, out: &mut Vec<Packet>) -> Result<()> {
        unsafe {
            match self.events.clone() {
                None => {
                    self.mft.ProcessInput(0, sample, 0)?;
                    self.drain(out)?;
                }
                Some(ev) => {
                    // pump until the encoder asks for input
                    loop {
                        while let Ok(e) = ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                            self.on_event(&e, out)?;
                        }
                        if self.need_input > 0 {
                            break;
                        }
                        let e = ev.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))?;
                        self.on_event(&e, out)?;
                    }
                    self.need_input -= 1;
                    self.mft.ProcessInput(0, sample, 0)?;
                    while let Ok(e) = ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                        self.on_event(&e, out)?;
                    }
                }
            }
        }
        Ok(())
    }

    unsafe fn on_event(&mut self, e: &IMFMediaEvent, out: &mut Vec<Packet>) -> Result<()> {
        let t = e.GetType()?;
        if t == METransformNeedInput.0 as u32 {
            self.need_input += 1;
        } else if t == METransformHaveOutput.0 as u32 {
            self.pull_one(out)?;
        }
        Ok(())
    }

    unsafe fn drain(&mut self, out: &mut Vec<Packet>) -> Result<()> {
        while self.pull_one(out)? {}
        Ok(())
    }

    /// One ProcessOutput call. Ok(false) = needs more input.
    unsafe fn pull_one(&mut self, out: &mut Vec<Packet>) -> Result<bool> {
        let alloc = if self.provides_samples {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
            Some(s)
        };
        let mut db = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(alloc),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let r = self.mft.ProcessOutput(0, &mut db, &mut status);
        let [mut d] = db;
        let sample = ManuallyDrop::take(&mut d.pSample);
        ManuallyDrop::drop(&mut d.pEvents);
        match r {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(false),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                let t = self.mft.GetOutputAvailableType(0, 0)?;
                self.mft.SetOutputType(0, &t, 0)?;
                return Ok(true);
            }
            Err(e) => return Err(e),
        }
        let Some(s) = sample else { return Ok(true) };
        let data = sample_bytes(&s)?;
        let pts = s.GetSampleTime().unwrap_or(0);
        let dur = s.GetSampleDuration().unwrap_or(10_000_000 / self.fps as i64);
        let key = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
        if key && self.hdr.is_none() {
            self.hdr = sps_pps(&data);
        }
        out.push(Packet { data: data.into(), pts, dur, key });
        Ok(true)
    }
}

impl Drop for VideoEnc {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
    }
}

// ===================================================================== audio

pub const A_RATE: u32 = 48_000;

pub struct AudioEnc {
    mft: IMFTransform,
    pub user_data: Vec<u8>,
    pub avg_bytes: u32,
    out_size: u32,
    provides: bool,
}

impl AudioEnc {
    pub fn new(bitrate_bytes: u32) -> Result<AudioEnc> {
        startup();
        let cands = enum_mfts(
            MFT_CATEGORY_AUDIO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            MFAudioFormat_PCM,
            MFMediaType_Audio,
            MFAudioFormat_AAC,
        );
        let a = cands.first().ok_or_else(|| windows::core::Error::from(E_FAIL))?;
        unsafe {
            let mft: IMFTransform = a.ActivateObject()?;
            let ot = MFCreateMediaType()?;
            ot.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            ot.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            ot.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            ot.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, A_RATE)?;
            ot.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            ot.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bitrate_bytes)?;
            ot.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
            ot.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
            mft.SetOutputType(0, &ot, 0)?;
            let it = MFCreateMediaType()?;
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            it.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            it.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            it.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, A_RATE)?;
            it.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            it.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
            it.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, A_RATE * 4)?;
            mft.SetInputType(0, &it, 0)?;
            let out_type = mft.GetOutputCurrentType(0)?;
            let n = out_type.GetBlobSize(&MF_MT_USER_DATA).unwrap_or(0);
            let mut user_data = vec![0u8; n as usize];
            if n > 0 {
                out_type.GetBlob(&MF_MT_USER_DATA, &mut user_data, None)?;
            }
            let info = mft.GetOutputStreamInfo(0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(AudioEnc {
                mft,
                user_data,
                avg_bytes: bitrate_bytes,
                out_size: info.cbSize.max(8192),
                provides: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
            })
        }
    }

    /// `pcm` = interleaved stereo s16 starting at `pts`.
    pub fn encode(&mut self, pcm: &[i16], pts: i64, out: &mut Vec<Packet>) -> Result<()> {
        unsafe {
            let bytes = std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2);
            let dur = (pcm.len() / 2) as i64 * 10_000_000 / A_RATE as i64;
            let s = mem_sample(bytes, pts, dur, false)?;
            self.mft.ProcessInput(0, &s, 0)?;
            loop {
                let alloc = if self.provides {
                    None
                } else {
                    let s = MFCreateSample()?;
                    s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
                    Some(s)
                };
                let mut db = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(alloc),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let r = self.mft.ProcessOutput(0, &mut db, &mut status);
                let [mut d] = db;
                let sample = ManuallyDrop::take(&mut d.pSample);
                ManuallyDrop::drop(&mut d.pEvents);
                match r {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) => return Err(e),
                }
                if let Some(s) = sample {
                    let data = sample_bytes(&s)?;
                    if !data.is_empty() {
                        out.push(Packet {
                            data: data.into(),
                            pts: s.GetSampleTime().unwrap_or(pts),
                            dur: s.GetSampleDuration().unwrap_or(213_333),
                            key: true,
                        });
                    }
                }
            }
        }
    }
}

// ===================================================================== mux

#[derive(Clone)]
pub struct VInfo {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub hdr: Vec<u8>,
}

#[derive(Clone)]
pub struct AInfo {
    pub user_data: Vec<u8>,
    pub avg_bytes: u32,
}

pub fn video_type(v: &VInfo) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, v.bitrate)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(v.w, v.h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pack(v.fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &v.hdr)?;
        Ok(t)
    }
}

pub fn audio_type(a: &AInfo) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, A_RATE)?;
        t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, a.avg_bytes)?;
        t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 1)?;
        t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
        t.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
        if !a.user_data.is_empty() {
            t.SetBlob(&MF_MT_USER_DATA, &a.user_data)?;
        }
        Ok(t)
    }
}

/// MP4 writer in pass-through mode: input type == output type, no transcoding.
pub struct Mp4 {
    w: IMFSinkWriter,
    v: u32,
    a: u32,
    t0: Option<i64>,
}

impl Mp4 {
    pub fn create(path: &std::path::Path, vi: &VInfo, ai: &AInfo) -> Result<Mp4> {
        startup();
        let vtype = &video_type(vi)?;
        let atype = &audio_type(ai)?;
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.unwrap();
            attrs.SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)?;
            attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            let url = windows::core::HSTRING::from(path.as_os_str());
            let w = MFCreateSinkWriterFromURL(&url, None, &attrs)?;
            let v = w.AddStream(vtype)?;
            w.SetInputMediaType(v, vtype, None)?;
            let a = w.AddStream(atype)?;
            w.SetInputMediaType(a, atype, None)?;
            w.BeginWriting()?;
            Ok(Mp4 { w, v, a, t0: None })
        }
    }

    pub fn write(&mut self, video: bool, p: &Packet) -> Result<()> {
        let t0 = *self.t0.get_or_insert(p.pts);
        let pts = (p.pts - t0).max(0);
        unsafe {
            let s = mem_sample(&p.data, pts, p.dur, p.key)?;
            self.w.WriteSample(if video { self.v } else { self.a }, &s)
        }
    }

    pub fn set_t0(&mut self, t0: i64) {
        self.t0 = Some(t0);
    }

    pub fn finish(self) -> Result<()> {
        unsafe { self.w.Finalize() }
    }
}
