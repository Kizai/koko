//! Read-only access to the interactive Windows desktop.
mod displays;
mod frame;
mod image;
#[cfg(test)]
mod tests;

use self::displays::{Monitor, monitors};
use koko_input::coordinates::PhysicalRect;
use koko_proto::{ImageFormat, RemoteDisplay};
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        UI::{
            HiDpi::{
                AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                GetDpiAwarenessContextForProcess, SetProcessDpiAwarenessContext,
            },
            WindowsAndMessaging::{
                GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN, SYSTEM_METRICS_INDEX,
            },
        },
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
    match unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } {
        Ok(()) => Ok(()),
        Err(error) => {
            // A manifest or earlier caller may already have set the process default.
            // Query the process, not a temporary thread override, and retain the
            // setter's original error if its required V2 context is not in place.
            let context = unsafe { GetDpiAwarenessContextForProcess(HANDLE::default()) };
            if unsafe {
                AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                    .as_bool()
            } {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

pub fn displays() -> windows::core::Result<Vec<RemoteDisplay>> {
    Ok(monitors()?
        .into_iter()
        .map(|monitor| monitor.display)
        .collect())
}

/// Resolve the stable monitor and use Windows' virtual-desktop normalization bounds.
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
    let monitor = PhysicalRect::new(
        target.bounds.left,
        target.bounds.top,
        target.display.width,
        target.display.height,
    );
    // MOUSEEVENTF_VIRTUALDESK uses the entire OS virtual screen, including any
    // monitor omitted from our selectable list due to missing display metadata.
    let desktop = virtual_desktop_rect(|index| unsafe { GetSystemMetrics(index) });
    Ok(monitor.zip(desktop))
}

fn virtual_desktop_rect(
    mut metric: impl FnMut(SYSTEM_METRICS_INDEX) -> i32,
) -> Option<PhysicalRect> {
    let x = metric(SM_XVIRTUALSCREEN);
    let y = metric(SM_YVIRTUALSCREEN);
    let width = metric(SM_CXVIRTUALSCREEN);
    let height = metric(SM_CYVIRTUALSCREEN);
    PhysicalRect::new(x, y, width.try_into().ok()?, height.try_into().ok()?)
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
