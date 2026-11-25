use crate::{DesktopBackend, DesktopRequest, Origin};
use base64::{Engine, engine::general_purpose::STANDARD};
use koko_capture::windows::{self as capture, CaptureError, Image};
use koko_input::{
    keyboard::KeyboardSource,
    windows::{Geometry, InputError, WindowsInput},
};
use koko_proto::{ImageFormat, KeyboardPlatform, RemoteCommand, RemoteReply};

pub struct WindowsDesktopBackend {
    name: String,
}

impl WindowsDesktopBackend {
    pub fn new(name: String) -> windows::core::Result<Self> {
        capture::enable_per_monitor_dpi()?;
        Ok(Self { name })
    }
}

impl DesktopBackend for WindowsDesktopBackend {
    fn handle(&mut self, request: &DesktopRequest) -> RemoteReply {
        use RemoteCommand::*;
        match &request.command {
            Displays => match capture::displays() {
                Ok(displays) if !displays.is_empty() => RemoteReply::Displays {
                    host_name: self.name.clone(),
                    displays,
                    video: false,
                    clipboard: false,
                    clipboard_stream: false,
                    control: None,
                },
                Ok(_) => RemoteReply::error("display_unavailable"),
                Err(error) => {
                    eprintln!("Display enumeration failed: {error}");
                    RemoteReply::error("desktop_unavailable")
                }
            },
            Screenshot => match capture::capture(None, 1600, 80, ImageFormat::Png, true) {
                Ok(image) => RemoteReply::Frame {
                    png: STANDARD.encode(image.bytes),
                },
                Err(error) => capture_error(error),
            },
            Capture {
                display,
                edge,
                quality,
                format,
                cursor,
            } => image_reply(capture::capture(
                Some(*display),
                *edge,
                *quality,
                format.unwrap_or(ImageFormat::Jpeg),
                cursor.unwrap_or(false),
            )),
            Wallpaper => image_reply(capture::wallpaper()),
            _ => RemoteReply::error("desktop_unavailable"),
        }
    }
}

/// Input is ordered on the console event loop, independently of desktop capture.
pub struct WindowsDesktopInput(WindowsInput);

impl WindowsDesktopInput {
    pub fn new(cross_system_shortcuts: bool) -> Self {
        Self(WindowsInput::new(cross_system_shortcuts))
    }

    pub fn release(&mut self) {
        let _ = input_reply(self.0.release());
    }
}

impl DesktopBackend for WindowsDesktopInput {
    fn handle(&mut self, request: &DesktopRequest) -> RemoteReply {
        // Phase 3 will apply live agent policy at this boundary. Even already
        // approved agent metadata cannot authorize input in phase 1.2.
        if !matches!(request.origin, Origin::Person) {
            return RemoteReply::error("input_failed");
        }
        use RemoteCommand::*;
        let result = match &request.command {
            Pointer {
                display,
                events,
                source_os,
            } => self.0.pointer(
                || input_geometry(Some(*display)),
                match source_os {
                    Some(KeyboardPlatform::Macos) => KeyboardSource::MacOs,
                    Some(KeyboardPlatform::Windows) => KeyboardSource::Windows,
                    _ => KeyboardSource::Other,
                },
                events,
            ),
            Click { x, y } => self.0.click(input_geometry(None), (*x, *y)),
            Key { key } => self.0.key(*key),
            Text { text } => self.0.text(text),
            _ => return RemoteReply::error("desktop_unavailable"),
        };
        input_reply(result)
    }
}

fn input_geometry(display: Option<u32>) -> Option<Geometry> {
    match capture::input_geometry(display) {
        Ok(bounds) => bounds.map(|(monitor, desktop)| Geometry { monitor, desktop }),
        Err(error) => {
            eprintln!("Could not resolve Windows input display: {error}");
            None
        }
    }
}

fn input_reply(result: Result<(), InputError>) -> RemoteReply {
    match result {
        Ok(()) => RemoteReply::Ok,
        Err(InputError::DisplayUnavailable) => RemoteReply::error("display_unavailable"),
        Err(InputError::Failed(message)) => {
            eprintln!("{message}");
            RemoteReply::error("input_failed")
        }
    }
}

fn image_reply(result: Result<Image, CaptureError>) -> RemoteReply {
    match result {
        Ok(image) => RemoteReply::Image {
            image: STANDARD.encode(image.bytes),
            width: Some(image.width),
            height: Some(image.height),
            format: Some(image.format),
        },
        Err(error) => capture_error(error),
    }
}

fn capture_error(error: CaptureError) -> RemoteReply {
    match error {
        CaptureError::DisplayUnavailable => RemoteReply::error("display_unavailable"),
        CaptureError::ScreenPermissionRequired => RemoteReply::error("screen_permission_required"),
        CaptureError::Failed(error) => {
            eprintln!("Image capture failed: {error}");
            RemoteReply::error("capture_failed")
        }
    }
}
