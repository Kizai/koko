//! Screen capture for Koko hosts.

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
pub fn check_screen_recording_permission(request: bool) -> bool {
    // Core Graphics checks the calling executable's identity.
    unsafe {
        if request && !CGPreflightScreenCaptureAccess() {
            CGRequestScreenCaptureAccess();
        }
        CGPreflightScreenCaptureAccess()
    }
}
