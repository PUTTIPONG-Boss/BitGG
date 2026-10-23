//! Screen capture the way Moments' gsdk `duplicator_monitor_capture` does it:
//! DXGI Desktop Duplication on the adapter that owns the monitor, then a
//! D3D11 video processor converts BGRA -> NV12 (and scales) on the GPU so
//! only ~1.5 bytes/pixel ever cross to the CPU.

use std::mem::ManuallyDrop;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::{HMODULE, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

#[derive(Clone, Debug)]
pub struct Monitor {
    pub adapter: u32,
    pub output: u32,
    pub name: String,
    pub w: u32,
    pub h: u32,
}

pub fn monitors() -> Vec<Monitor> {
    let mut v = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return v };
        let mut a = 0;
        while let Ok(ad) = f.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(out) = ad.EnumOutputs(o) {
                if let Ok(d) = out.GetDesc() {
                    if d.AttachedToDesktop.as_bool() {
                        let r = d.DesktopCoordinates;
                        let name = String::from_utf16_lossy(&d.DeviceName)
                            .trim_end_matches('\0')
                            .trim_start_matches("\\\\.\\")
                            .to_string();
                        v.push(Monitor {
                            adapter: a,
                            output: o,
                            name,
                            w: (r.right - r.left) as u32,
                            h: (r.bottom - r.top) as u32,
                        });
                    }
                }
                o += 1;
            }
            a += 1;
        }
    }
    v
}

pub struct Grabber {
    ctx: ID3D11DeviceContext,
    output: IDXGIOutput1,
    device: ID3D11Device,
    dup: Option<IDXGIOutputDuplication>,
    src: ID3D11Texture2D,
    vctx: ID3D11VideoContext,
    vp: ID3D11VideoProcessor,
    in_view: ID3D11VideoProcessorInputView,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    staging: ID3D11Texture2D,
    pub out_w: u32,
    pub out_h: u32,
    src_w: u32,
    src_h: u32,
}

fn tex(dev: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT, usage: D3D11_USAGE, bind: u32, cpu: u32) -> Result<ID3D11Texture2D> {
    let d = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { dev.CreateTexture2D(&d, None, Some(&mut t))? };
    Ok(t.unwrap())
}

impl Grabber {
    /// `height` = target output height (0 = native). Width keeps aspect.
    pub fn new(mon: &Monitor, height: u32, fps: u32) -> Result<Grabber> {
        unsafe {
            let f = CreateDXGIFactory1::<IDXGIFactory1>()?;
            let ad = f.EnumAdapters1(mon.adapter)?;
            let output: IDXGIOutput1 = ad.EnumOutputs(mon.output)?.cast()?;
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &ad,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device = device.unwrap();
            let ctx = ctx.unwrap();
            let (src_w, src_h) = (mon.w, mon.h);
            let (mut out_w, mut out_h) = if height == 0 || height >= src_h {
                (src_w, src_h)
            } else {
                (src_w * height / src_h, height)
            };
            out_w &= !1;
            out_h &= !1;

            let src = tex(&device, src_w, src_h, DXGI_FORMAT_B8G8R8A8_UNORM, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
            let staging = tex(&device, out_w, out_h, DXGI_FORMAT_NV12, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32)?;

            let vdev: ID3D11VideoDevice = device.cast()?;
            let vctx: ID3D11VideoContext = ctx.cast()?;
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                InputWidth: src_w,
                InputHeight: src_h,
                OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let en = vdev.CreateVideoProcessorEnumerator(&desc)?;
            let vp = vdev.CreateVideoProcessor(&en, 0)?;
            let ivd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut in_view = None;
            vdev.CreateVideoProcessorInputView(&src, &en, &ivd, Some(&mut in_view))?;
            let ovd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            // ring of NV12 surfaces: the async encoder may still be reading older ones
            let mut pool = Vec::new();
            for _ in 0..6 {
                let t = tex(&device, out_w, out_h, DXGI_FORMAT_NV12, D3D11_USAGE_DEFAULT, D3D11_BIND_RENDER_TARGET.0 as u32, 0)?;
                let mut v = None;
                vdev.CreateVideoProcessorOutputView(&t, &en, &ovd, Some(&mut v))?;
                pool.push((t, v.unwrap()));
            }

            // full-range RGB in, BT.709 limited-range YCbCr out
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 });
            vctx.VideoProcessorSetOutputColorSpace(&vp, &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) });
            let full = RECT { left: 0, top: 0, right: src_w as i32, bottom: src_h as i32 };
            let dst = RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 };
            vctx.VideoProcessorSetStreamSourceRect(&vp, 0, TRUE, Some(&full));
            vctx.VideoProcessorSetStreamDestRect(&vp, 0, TRUE, Some(&dst));
            vctx.VideoProcessorSetOutputTargetRect(&vp, TRUE, Some(&dst));
            vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);

            let mut g = Grabber {
                ctx,
                output,
                device,
                dup: None,
                src,
                vctx,
                vp,
                in_view: in_view.unwrap(),
                pool,
                next: 0,
                staging,
                out_w,
                out_h,
                src_w,
                src_h,
            };
            g.reopen()?;
            Ok(g)
        }
    }

    fn reopen(&mut self) -> Result<()> {
        self.dup = None;
        unsafe {
            self.dup = Some(self.output.DuplicateOutput(&self.device)?);
        }
        Ok(())
    }

    /// Grab a new desktop frame if one arrived within `timeout_ms`.
    /// Returns Ok(false) when nothing changed (caller re-sends the last frame).
    pub fn grab(&mut self, timeout_ms: u32) -> Result<bool> {
        if self.dup.is_none() {
            if self.reopen().is_err() {
                std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
                return Ok(false);
            }
        }
        let dup = self.dup.as_ref().unwrap();
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        unsafe {
            match dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => {
                    self.dup = None; // fullscreen switch / mode change / UAC
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
            let mut fresh = false;
            if info.LastPresentTime != 0 {
                if let Some(r) = res {
                    let t: ID3D11Texture2D = r.cast()?;
                    let mut d = D3D11_TEXTURE2D_DESC::default();
                    t.GetDesc(&mut d);
                    if d.Width == self.src_w && d.Height == self.src_h {
                        self.ctx.CopyResource(&self.src, &t);
                        fresh = true;
                    }
                }
            }
            let _ = dup.ReleaseFrame();
            Ok(fresh)
        }
    }

    pub fn device(&self) -> &ID3D11Device {
        &self.device
    }

    /// Convert the last grabbed frame to NV12 on the GPU; returns the surface.
    pub fn convert(&mut self) -> Result<ID3D11Texture2D> {
        let (t, ov) = self.pool[self.next].clone();
        self.next = (self.next + 1) % self.pool.len();
        unsafe {
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: TRUE,
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(Some(self.in_view.clone())),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let streams = [stream];
            let r = self.vctx.VideoProcessorBlt(&self.vp, &ov, 0, &streams);
            let [mut s] = streams;
            ManuallyDrop::drop(&mut s.pInputSurface);
            r?;
        }
        Ok(t)
    }

    /// CPU fallback for software encoders: NV12 bytes of the last frame.
    pub fn read_nv12(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let t = self.convert()?;
        out.clear();
        unsafe {
            self.ctx.CopyResource(&self.staging, &t);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let pitch = m.RowPitch as usize;
            let base = m.pData as *const u8;
            let w = self.out_w as usize;
            let h = self.out_h as usize;
            for y in 0..h {
                out.extend_from_slice(std::slice::from_raw_parts(base.add(y * pitch), w));
            }
            let uv = base.add(pitch * h);
            for y in 0..h / 2 {
                out.extend_from_slice(std::slice::from_raw_parts(uv.add(y * pitch), w));
            }
            self.ctx.Unmap(&self.staging, 0);
        }
        Ok(())
    }
}
