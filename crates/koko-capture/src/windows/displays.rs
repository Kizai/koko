use crate::image_geometry::display_id;
use koko_proto::RemoteDisplay;
use std::collections::{HashMap, HashSet};
use windows::{
    Win32::{
        Devices::Display::*,
        Foundation::{E_FAIL, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT, WIN32_ERROR},
        Graphics::Gdi::*,
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
    core::{BOOL, Result},
};

pub(super) struct Monitor {
    pub handle: HMONITOR,
    pub bounds: RECT,
    pub primary: bool,
    pub display: RemoteDisplay,
}

pub(super) fn monitors() -> Result<Vec<Monitor>> {
    let names = active_targets()?;
    let mut monitors: Vec<(HMONITOR, MONITORINFOEXW)> = Vec::new();
    // EnumDisplayMonitors is synchronous and the callback cannot outlive this vector.
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&raw mut monitors) as isize),
        )
        .ok()?;
    }

    let mut ids = HashSet::new();
    let mut result = Vec::new();
    for (handle, info) in monitors {
        let source = utf16_name(&info.szDevice);
        // GDI also reports pseudo-monitors from mirroring drivers.
        let Some((path, name)) = names.get(&source.to_ascii_lowercase()) else {
            continue;
        };
        let id = display_id(path);
        if !ids.insert(id) {
            return Err(E_FAIL.into());
        }
        let bounds = info.monitorInfo.rcMonitor;
        result.push(Monitor {
            handle,
            bounds,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            display: RemoteDisplay {
                id,
                name: if name.is_empty() {
                    source
                } else {
                    name.clone()
                },
                width: (bounds.right - bounds.left) as u32,
                height: (bounds.bottom - bounds.top) as u32,
            },
        });
    }
    result.sort_by_key(|monitor| (!monitor.primary, monitor.display.id));
    Ok(result)
}

unsafe extern "system" fn collect_monitor(
    handle: HMONITOR,
    _: HDC,
    _: *mut RECT,
    context: LPARAM,
) -> BOOL {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    unsafe {
        if !GetMonitorInfoW(handle, &raw mut info.monitorInfo).as_bool() {
            return false.into();
        }
        (*(context.0 as *mut Vec<(HMONITOR, MONITORINFOEXW)>)).push((handle, info));
    }
    true.into()
}

fn active_targets() -> Result<HashMap<String, (String, String)>> {
    let mut names = HashMap::<String, (String, String)>::new();
    for path in active_paths()? {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            },
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            },
            ..Default::default()
        };
        unsafe {
            WIN32_ERROR(DisplayConfigGetDeviceInfo(&raw mut source.header) as u32).ok()?;
            WIN32_ERROR(DisplayConfigGetDeviceInfo(&raw mut target.header) as u32).ok()?;
        }
        let device_path = utf16_name(&target.monitorDevicePath).to_ascii_lowercase();
        if device_path.is_empty() {
            continue;
        }
        let key = utf16_name(&source.viewGdiDeviceName).to_ascii_lowercase();
        // A cloned source produces one capturable desktop. Choose a stable
        // target independently of the order returned by QueryDisplayConfig.
        if names.get(&key).is_none_or(|(path, _)| device_path < *path) {
            names.insert(
                key,
                (device_path, utf16_name(&target.monitorFriendlyDeviceName)),
            );
        }
    }
    Ok(names)
}

fn active_paths() -> Result<Vec<DISPLAYCONFIG_PATH_INFO>> {
    // The topology can change between the sizing call and the query.
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0, 0);
        unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
                .ok()?;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        match status {
            ERROR_INSUFFICIENT_BUFFER => continue,
            ERROR_SUCCESS => {
                paths.truncate(path_count as usize);
                return Ok(paths);
            }
            error => error.ok()?,
        }
    }
    Err(ERROR_INSUFFICIENT_BUFFER.to_hresult().into())
}

fn utf16_name(value: &[u16]) -> String {
    let end = value.iter().position(|&ch| ch == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}
