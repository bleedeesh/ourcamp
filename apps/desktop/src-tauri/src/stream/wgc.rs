//! A window through Windows.Graphics.Capture directly, not through libwebrtc: its capturer
//! spends a frame's whole time on a call and gives a game 30 frames where the system offers
//! 200 (ADR-0060).
//!
//! ```text
//! the system ─ frame pool ─► oc-stream-capture: the newest frame at the chosen pace
//!                            ─ copy on the video card ─ read into memory ─► the callback
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Once};
use std::time::{Duration, Instant};

use windows::core::{factory, IInspectable, Interface, Result as WinResult};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
/// One frame held for its moment, one on the way, one being drawn by the system.
const POOL_FRAMES: i32 = 3;
/// With no frame in hand the wait ends this often, to notice being told to stop.
const IDLE: Duration = Duration::from_millis(50);

/// A picture in memory: BGRA, what libyuv calls ARGB.
pub(super) struct Picture<'a> {
    pub data: &'a [u8],
    pub stride: i32,
    pub width: i32,
    pub height: i32,
}

/// The capture of one window. Lives and dies on the thread that opened it.
pub(super) struct WindowCapture {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
    item: GraphicsCaptureItem,
    tick: Duration,
    running: Running,
    signal: mpsc::Sender<()>,
    arrived: mpsc::Receiver<()>,
    /// Where a frame is copied to be read; remade when frames change size.
    staging: Option<ID3D11Texture2D>,
}

/// A frame pool and its session, made for one size of the window.
struct Running {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    token: i64,
    size: SizeInt32,
}

impl Running {
    fn start(
        winrt: &IDirect3DDevice,
        item: &GraphicsCaptureItem,
        size: SizeInt32,
        tick: Duration,
        signal: mpsc::Sender<()>,
    ) -> WinResult<Self> {
        let pool =
            Direct3D11CaptureFramePool::CreateFreeThreaded(winrt, FORMAT, POOL_FRAMES, size)?;
        let session = pool.CreateCaptureSession(item)?;
        // Older systems lack these, and a stream works without them: with a yellow border
        // around the window, and with more frames offered than the pace takes.
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetMinUpdateInterval(TimeSpan {
            // In units of 100 ns. Half a frame, so the newest is never far behind the pace.
            Duration: (tick.as_nanos() / 200) as i64,
        });
        let token = pool.FrameArrived(&TypedEventHandler::<
            Direct3D11CaptureFramePool,
            IInspectable,
        >::new(move |_, _| {
            let _ = signal.send(());
            Ok(())
        }))?;
        session.StartCapture()?;
        Ok(Self {
            pool,
            session,
            token,
            size,
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

impl WindowCapture {
    /// Starts capturing `window`. An error means this way will not work here: the system is
    /// too old, there is no such window, the video card refused.
    pub(super) fn open(window: HWND, tick: Duration) -> WinResult<Self> {
        keep_com_loaded();
        let (device, context) = device()?;
        let dxgi: IDXGIDevice = device.cast()?;
        // SAFETY: `dxgi` is a live device of ours; the call only wraps it.
        let winrt: IDirect3DDevice =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }?.cast()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        // SAFETY: any value is safe to pass as a handle: a wrong one comes back as an error.
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(window) }?;
        let size = item.Size()?;
        let (signal, arrived) = mpsc::channel();
        let running = Running::start(&winrt, &item, size, tick, signal.clone())?;
        Ok(Self {
            device,
            context,
            winrt,
            item,
            tick,
            running,
            signal,
            arrived,
            staging: None,
        })
    }

    /// Hands over the newest frame at most once a `tick`, until told to stop. A window that
    /// does not change gives no frames: the pump repeats the last one. An error means the
    /// capture cannot go on, the video card was lost or reset.
    pub(super) fn run(
        &mut self,
        stop: &AtomicBool,
        mut deliver: impl FnMut(&Picture<'_>),
    ) -> WinResult<()> {
        // A frame is kept until its moment, not thrown away and waited for again: with a game
        // at this same rate that would halve the rate.
        let mut held: Option<Direct3D11CaptureFrame> = None;
        let mut next = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let wait = if held.is_some() {
                next.saturating_duration_since(Instant::now())
            } else {
                IDLE
            };
            let _ = self.arrived.recv_timeout(wait);
            self.arrived.try_iter().for_each(drop);
            // Closed at once: the pool has only so many, and it stops when they are all held.
            while let Ok(frame) = self.running.pool.TryGetNextFrame() {
                if let Some(older) = held.replace(frame) {
                    older.Close()?;
                }
            }
            let now = Instant::now();
            if now < next {
                continue;
            }
            let Some(frame) = held.take() else {
                continue;
            };
            let content = frame.ContentSize()?;
            if content != self.running.size && content.Width >= 2 && content.Height >= 2 {
                // A window that stays still would never fill a bigger pool: only a new session
                // promises a first frame, and at the new size.
                frame.Close()?;
                self.running = Running::start(
                    &self.winrt,
                    &self.item,
                    content,
                    self.tick,
                    self.signal.clone(),
                )?;
                continue;
            }
            let read = self.read(&frame, &mut deliver);
            frame.Close()?;
            read?;
            next += self.tick;
            if next < now {
                // Behind: drop the debt, do not rush to catch up.
                next = now;
            }
        }
        if let Some(frame) = held {
            frame.Close()?;
        }
        Ok(())
    }

    fn read(
        &mut self,
        frame: &Direct3D11CaptureFrame,
        deliver: &mut impl FnMut(&Picture<'_>),
    ) -> WinResult<()> {
        let content = frame.ContentSize()?;
        if content.Width < 2 || content.Height < 2 {
            // A minimized window.
            return Ok(());
        }
        let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        // SAFETY: the surface is alive for this call and we asked for its own texture type.
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` outlives the call.
        unsafe { texture.GetDesc(&raw mut desc) };
        let staging = self.staging_for(&desc)?;
        let width = (content.Width as u32).min(desc.Width);
        let height = (content.Height as u32).min(desc.Height);
        // Only the content: past it, a buffer bigger than the window holds old pixels.
        let area = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are alive and have this format, the area fits both, and
        // `map` outlives the mapping, which ends before this block does; the slice covers
        // only the rows read here, each `RowPitch` long, and is not kept by `deliver`.
        unsafe {
            self.context.CopySubresourceRegion(
                &staging,
                0,
                0,
                0,
                0,
                &texture,
                0,
                Some(&raw const area),
            );
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&raw mut map))?;
            deliver(&Picture {
                data: std::slice::from_raw_parts(
                    map.pData.cast::<u8>(),
                    (map.RowPitch * height) as usize,
                ),
                stride: map.RowPitch.cast_signed(),
                width: width.cast_signed(),
                height: height.cast_signed(),
            });
            self.context.Unmap(&staging, 0);
        }
        Ok(())
    }

    /// The texture to read from, the size of the frames that come now.
    fn staging_for(&mut self, frame: &D3D11_TEXTURE2D_DESC) -> WinResult<ID3D11Texture2D> {
        if let Some(staging) = &self.staging {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `desc` outlives the call.
            unsafe { staging.GetDesc(&raw mut desc) };
            if (desc.Width, desc.Height) == (frame.Width, frame.Height) {
                return Ok(staging.clone());
            }
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..*frame
        };
        let mut made = None;
        // SAFETY: `desc` and `made` outlive the call.
        unsafe {
            self.device
                .CreateTexture2D(&raw const desc, None, Some(&raw mut made))
        }?;
        let made = made.ok_or_else(windows::core::Error::empty)?;
        self.staging = Some(made.clone());
        Ok(made)
    }
}

/// A device on the video card, with what capture needs from it.
/// When the last thread leaves COM, Windows unloads `GraphicsCapture.dll` while its own worker
/// may still be finishing, and the whole process dies (ADR-0060). One usage of the
/// multithreaded apartment, never given back, keeps it loaded until the process ends.
fn keep_com_loaded() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: no arguments and no thread state; the cookie is only for giving it back.
        if let Err(e) = unsafe { CoIncrementMTAUsage() } {
            eprintln!("[oc-stream] keeping COM: {e}");
        }
    });
}

fn device() -> WinResult<(ID3D11Device, ID3D11DeviceContext)> {
    let (mut device, mut context) = (None, None);
    // SAFETY: the out parameters outlive the call, and the rest are plain values.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&raw mut device),
            None,
            Some(&raw mut context),
        )
    }?;
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(windows::core::Error::empty()),
    }
}
