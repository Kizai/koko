//! Read-only access to the interactive Windows desktop.
mod displays;
mod frame;
mod image;

use self::displays::{Monitor, monitors};
use koko_input::coordinates::PhysicalRect;
use koko_proto::{ImageFormat, RemoteDisplay};
use windows::{
    Win32::{
        Foundation::RECT,
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
    },
    core::{Error, HRESULT},
};

pub struct Image {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
}

#[derive(Debug)]
pub enum CaptureError {
    DisplayUnavailable,
    ScreenPermissionRequired,
    Failed(Error),
}

impl From<Error> for CaptureError {
    fn from(error: Error) -> Self {
        if error.code() == HRESULT::from_win32(5) {
            Self::ScreenPermissionRequired
        } else {
            Self::Failed(error)
        }
    }
}

/// Call at process startup before enumerating or capturing displays.
pub fn enable_per_monitor_dpi() -> windows::core::Result<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
}

pub fn displays() -> windows::core::Result<Vec<RemoteDisplay>> {
    Ok(monitors()?
        .into_iter()
        .map(|monitor| monitor.display)
        .collect())
}

/// Resolve the stable monitor ID and virtual desktop from the same topology snapshot.
pub fn input_geometry(
    display: Option<u32>,
) -> windows::core::Result<Option<(PhysicalRect, PhysicalRect)>> {
    let monitors = monitors()?;
    let Some(target) = monitors
        .iter()
        .find(|monitor| display.map_or(monitor.primary, |id| id == monitor.display.id))
    else {
        return Ok(None);
    };
    let bounds = monitors
        .iter()
        .fold(target.bounds, |desktop, monitor| RECT {
            left: desktop.left.min(monitor.bounds.left),
            top: desktop.top.min(monitor.bounds.top),
            right: desktop.right.max(monitor.bounds.right),
            bottom: desktop.bottom.max(monitor.bounds.bottom),
        });
    let monitor = PhysicalRect::new(
        target.bounds.left,
        target.bounds.top,
        target.display.width,
        target.display.height,
    );
    let desktop = PhysicalRect::new(
        bounds.left,
        bounds.top,
        bounds.right.abs_diff(bounds.left),
        bounds.bottom.abs_diff(bounds.top),
    );
    Ok(monitor.zip(desktop))
}

pub fn capture(
    display: Option<u32>,
    edge: u16,
    quality: u8,
    format: ImageFormat,
    cursor: bool,
) -> Result<Image, CaptureError> {
    let monitor = find_monitor(display)?;
    let _apartment = Apartment::new()?;
    let pixels = frame::capture(monitor.handle, cursor)?;
    Ok(image::encode_pixels(&pixels, edge, quality, format)?)
}

pub fn wallpaper() -> Result<Image, CaptureError> {
    let monitor = find_monitor(None)?;
    let _apartment = Apartment::new()?;
    Ok(image::wallpaper(monitor.bounds)?)
}

fn find_monitor(display: Option<u32>) -> Result<Monitor, CaptureError> {
    monitors()?
        .into_iter()
        .find(|monitor| display.map_or(monitor.primary, |id| id == monitor.display.id))
        .ok_or(CaptureError::DisplayUnavailable)
}

// Keep the apartment on the same thread as the WinRT and WIC calls.
struct Apartment;

impl Apartment {
    fn new() -> windows::core::Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}
