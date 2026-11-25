use super::*;
use std::process::Command;
use windows::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, HANDLE},
    UI::{
        HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
            GetDpiAwarenessContextForProcess,
        },
        WindowsAndMessaging::{
            SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        },
    },
};

#[test]
fn virtual_desktop_uses_all_four_system_metrics() {
    // Derived geometry, not a captured macOS reply. The OS virtual desktop
    // includes pixels outside the displays retained by the selectable list.
    let mut queried = Vec::new();
    let desktop = virtual_desktop_rect(|index| {
        queried.push(index.0);
        match index {
            SM_XVIRTUALSCREEN => -1920,
            SM_YVIRTUALSCREEN => -1200,
            SM_CXVIRTUALSCREEN => 4480,
            SM_CYVIRTUALSCREEN => 2640,
            _ => panic!("unexpected system metric"),
        }
    })
    .unwrap();
    assert_eq!(queried, [76, 77, 78, 79]);
    assert_eq!(desktop.pixel_to_absolute((-1920, -1200)), Some((0, 0)));
    assert_eq!(
        desktop.pixel_to_absolute((2559, 1439)),
        Some((65535, 65535))
    );
    assert_eq!(desktop.pixel_to_absolute((2560, 1439)), None);
}

#[test]
fn invalid_virtual_desktop_extents_are_not_normalized() {
    for (width, height) in [(0, 1080), (-1, 1080), (1920, 0), (1920, -1)] {
        assert!(
            virtual_desktop_rect(|index| match index {
                SM_XVIRTUALSCREEN | SM_YVIRTUALSCREEN => 0,
                SM_CXVIRTUALSCREEN => width,
                SM_CYVIRTUALSCREEN => height,
                _ => panic!("unexpected system metric"),
            })
            .is_none()
        );
    }
}

#[test]
fn dpi_initialization_accepts_only_existing_per_monitor_v2() {
    // DPI awareness is process-wide and cannot be reset. Each case therefore
    // runs in a fresh process; none enumerates or captures a desktop.
    for case in ["per_monitor_v2", "per_monitor_v1", "system"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows::tests::dpi_context_child",
                "--ignored",
                "--nocapture",
            ])
            .env("KOKO_DPI_CONTEXT_TEST", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "isolated fixture invoked by dpi_initialization_accepts_only_existing_per_monitor_v2"]
fn dpi_context_child() {
    let case = std::env::var("KOKO_DPI_CONTEXT_TEST").expect("launched by parent test");
    let context = match case.as_str() {
        "per_monitor_v2" => DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        "per_monitor_v1" => DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
        "system" => DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
        _ => panic!("invalid fixture case"),
    };
    unsafe { SetProcessDpiAwarenessContext(context).unwrap() };
    if case == "per_monitor_v2" {
        enable_per_monitor_dpi().unwrap();
        enable_per_monitor_dpi().unwrap();
        assert!(unsafe {
            AreDpiAwarenessContextsEqual(
                GetDpiAwarenessContextForProcess(HANDLE::default()),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
            .as_bool()
        });
    } else {
        let original =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                .unwrap_err();
        assert_eq!(original.code(), ERROR_ACCESS_DENIED.to_hresult());
        assert_eq!(
            enable_per_monitor_dpi().unwrap_err().code(),
            original.code()
        );
    }
}
