use crate::{DesktopBackend, DesktopRequest};
use base64::{Engine, engine::general_purpose::STANDARD};
use koko_capture::windows::{self as capture, CaptureError, Image};
use koko_proto::{ImageFormat, RemoteCommand, RemoteReply};

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
            // Phase 1.1 implements viewing only, for every authorized peer.
            _ => RemoteReply::error("desktop_unavailable"),
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
