use std::{slice, sync::mpsc, time::Duration};
use windows::{
    Foundation::TypedEventHandler,
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::{E_FAIL, E_NOTIMPL, ERROR_TIMEOUT},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, Dxgi::IDXGIDevice, Gdi::HMONITOR,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
    },
    core::{Interface, Result, factory},
};

pub(super) struct Pixels {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub(super) fn capture(monitor: HMONITOR, cursor: bool) -> Result<Pixels> {
    if !GraphicsCaptureSession::IsSupported()? {
        return Err(E_NOTIMPL.into());
    }
    let mut device = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )?;
    }
    let device = device.ok_or(E_FAIL)?;
    let dxgi: IDXGIDevice = device.cast()?;
    let winrt: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()?;
    let factory = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { factory.CreateForMonitor(monitor)? };
    let mut capture = SingleFrame {
        pool: Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            item.Size()?,
        )?,
        token: None,
        session: None,
    };
    let (sender, ready) = mpsc::sync_channel(1);
    capture.token = Some(
        capture
            .pool
            .FrameArrived(&TypedEventHandler::new(move |_, _| {
                let _ = sender.try_send(());
                Ok(())
            }))?,
    );

    capture.session = Some(capture.pool.CreateCaptureSession(&item)?);
    let session = capture.session.as_ref().unwrap();
    session.SetIsCursorCaptureEnabled(cursor)?;
    session.StartCapture()?;
    ready
        .recv_timeout(Duration::from_secs(4))
        .map_err(|_| ERROR_TIMEOUT.to_hresult())?;

    let frame = capture.pool.TryGetNextFrame()?;
    let pixels = read_pixels(&device, &frame);
    let _ = frame.Close();
    pixels
}

struct SingleFrame {
    pool: Direct3D11CaptureFramePool,
    token: Option<i64>,
    session: Option<GraphicsCaptureSession>,
}

impl Drop for SingleFrame {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = self.pool.RemoveFrameArrived(token);
        }
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
        let _ = self.pool.Close();
    }
}

fn read_pixels(device: &ID3D11Device, frame: &Direct3D11CaptureFrame) -> Result<Pixels> {
    let size = frame.ContentSize()?;
    let surface: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    unsafe {
        let texture: ID3D11Texture2D = surface.GetInterface()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let (width, height) = (size.Width as u32, size.Height as u32);
        // During a resolution increase WGC can report a content size larger than the
        // pool texture. Fail this request rather than returning a partial screenshot.
        if width == 0 || height == 0 || width > desc.Width || height > desc.Height {
            return Err(E_FAIL.into());
        }
        let mut staging = None;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            },
            None,
            Some(&mut staging),
        )?;
        let staging = staging.ok_or(E_FAIL)?;
        let context = device.GetImmediateContext()?;
        let source = D3D11_BOX {
            right: width,
            bottom: height,
            back: 1,
            ..Default::default()
        };
        context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&source));

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let stride = width as usize * 4;
        let mut bytes = Vec::with_capacity(stride * height as usize);
        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        // D3D owns the memory between Map and Unmap. RowPitch includes GPU padding.
        for row in 0..height as usize {
            let start = mapped
                .pData
                .cast::<u8>()
                .add(row * mapped.RowPitch as usize);
            bytes.extend_from_slice(slice::from_raw_parts(start, stride));
        }
        context.Unmap(&staging, 0);
        Ok(Pixels {
            bytes,
            width,
            height,
        })
    }
}
