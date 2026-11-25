use super::{Button, Control, ForegroundIntegrity, Geometry, Input, InputError, Record, Sink};
use crate::keyboard::KeyboardSource;
use koko_proto::{PointerEvent, RemoteKey};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows::{
    Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE},
        Security::{
            GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsValidSid,
            TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
        },
        System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::{
                GetForegroundWindow, GetWindowThreadProcessId, XBUTTON1, XBUTTON2,
            },
        },
    },
    core::{Error, Result as WindowsResult},
};

pub struct WindowsInput(Input<Win32>);

impl WindowsInput {
    pub fn new(cross_system_shortcuts: bool) -> Self {
        let integrity = integrity_level(unsafe { GetCurrentProcess() }).ok();
        Self(Input::new(Win32 { integrity }, cross_system_shortcuts))
    }

    pub fn pointer(
        &mut self,
        geometry: impl FnOnce() -> Option<Geometry>,
        source: KeyboardSource,
        events: &[PointerEvent],
    ) -> Result<(), InputError> {
        self.0.pointer(geometry, source, events)
    }

    pub fn click(
        &mut self,
        geometry: Option<Geometry>,
        point: (u16, u16),
    ) -> Result<(), InputError> {
        self.0.click(geometry, point)
    }

    pub fn key(&mut self, key: RemoteKey) -> Result<(), InputError> {
        self.0.key(key)
    }

    pub fn text(&mut self, text: &str) -> Result<(), InputError> {
        self.0.text(text)
    }

    pub fn release(&mut self) -> Result<(), InputError> {
        self.0.release()
    }
}

struct Win32 {
    integrity: Option<u32>,
}

impl Sink for Win32 {
    fn foreground_integrity(&self) -> Option<ForegroundIntegrity> {
        let (window, mut pid) = (unsafe { GetForegroundWindow() }, 0);
        // NULL is normal during activation transitions. Let a click recover focus.
        // If a comparison is unavailable, SendInput still enforces Windows UIPI.
        if window.0.is_null() {
            return None;
        }
        let host = self.integrity?;
        if unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) } == 0 {
            return None;
        }
        let target = (|| {
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)? };
            let process = unsafe { OwnedHandle::from_raw_handle(process.0) };
            integrity_level(HANDLE(process.as_raw_handle()))
        })()
        .ok()?;
        Some(ForegroundIntegrity {
            host,
            target,
            process_id: pid,
        })
    }

    fn send(&mut self, records: &[Record]) -> usize {
        let inputs: Vec<_> = records.iter().copied().map(native_input).collect();
        unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) as usize }
    }
}

fn integrity_level(process: HANDLE) -> WindowsResult<u32> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token)? };
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let handle = HANDLE(token.as_raw_handle());
    let mut bytes = 0;
    let error = unsafe { GetTokenInformation(handle, TokenIntegrityLevel, None, 0, &mut bytes) }
        .expect_err("a zero-length buffer cannot contain a mandatory label");
    if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
        return Err(error);
    }
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    let label = buffer.as_mut_ptr().cast::<TOKEN_MANDATORY_LABEL>();
    // The pointer-aligned buffer owns both the label and SID for this scope.
    unsafe {
        GetTokenInformation(
            handle,
            TokenIntegrityLevel,
            Some(label.cast()),
            bytes,
            &mut bytes,
        )?;
        let sid = (*label).Label.Sid;
        if sid.0.is_null() || !IsValidSid(sid).as_bool() {
            return Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
        Ok(*GetSidSubAuthority(sid, u32::from(count - 1)))
    }
}

fn native_input(record: Record) -> INPUT {
    let mouse = match record {
        Record::Move(x, y) => MOUSEINPUT {
            dx: i32::from(x),
            dy: i32::from(y),
            dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            ..Default::default()
        },
        Record::Wheel { horizontal, delta } => MOUSEINPUT {
            mouseData: delta as u32,
            dwFlags: if horizontal {
                MOUSEEVENTF_HWHEEL
            } else {
                MOUSEEVENTF_WHEEL
            },
            ..Default::default()
        },
        Record::Edge(Control::Button(button), down) => {
            let (press, release, data) = match button {
                Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
                Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
                Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
                Button::Back => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
                Button::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
            };
            MOUSEINPUT {
                mouseData: data,
                dwFlags: if down { press } else { release },
                ..Default::default()
            }
        }
        Record::Edge(control, down) => {
            let (scan, mut flags) = match control {
                Control::Scan(scan) => {
                    let mut flags = KEYEVENTF_SCANCODE;
                    if scan.extended {
                        flags |= KEYEVENTF_EXTENDEDKEY;
                    }
                    (u16::from(scan.make), flags)
                }
                Control::Unicode(unit) => (unit, KEYEVENTF_UNICODE),
                Control::Button(_) => unreachable!(),
            };
            if !down {
                flags |= KEYEVENTF_KEYUP;
            }
            return INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: scan,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            };
        }
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: mouse },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::Set1ScanCode;

    #[test]
    fn reads_own_process_integrity_without_injecting_input() {
        integrity_level(unsafe { GetCurrentProcess() }).unwrap();
    }

    #[test]
    fn encodes_win32_records_without_injecting_input() {
        let left = native_input(Record::Move(0, 65535));
        assert_eq!(left.r#type, INPUT_MOUSE);
        let left = unsafe { left.Anonymous.mi };
        assert_eq!((left.dx, left.dy), (0, 65535));
        assert_eq!(
            left.dwFlags,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK
        );

        for (button, data) in [(Button::Back, XBUTTON1), (Button::Forward, XBUTTON2)] {
            let up = native_input(Record::Edge(Control::Button(button), false));
            let up = unsafe { up.Anonymous.mi };
            assert_eq!(up.dwFlags, MOUSEEVENTF_XUP);
            assert_eq!(up.mouseData, u32::from(data));
        }

        let wheel = native_input(Record::Wheel {
            horizontal: true,
            delta: -10,
        });
        let wheel = unsafe { wheel.Anonymous.mi };
        assert_eq!(wheel.dwFlags, MOUSEEVENTF_HWHEEL);
        assert_eq!(wheel.mouseData as i32, -10);

        let arrow = native_input(Record::Edge(
            Control::Scan(Set1ScanCode {
                make: 0x4d,
                extended: true,
            }),
            false,
        ));
        assert_eq!(arrow.r#type, INPUT_KEYBOARD);
        let arrow = unsafe { arrow.Anonymous.ki };
        assert_eq!(arrow.wScan, 0x4d);
        assert_eq!(
            arrow.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY
        );

        let unicode = native_input(Record::Edge(Control::Unicode(0xd83d), true));
        let unicode = unsafe { unicode.Anonymous.ki };
        assert_eq!(unicode.wVk.0, 0);
        assert_eq!(unicode.wScan, 0xd83d);
        assert_eq!(unicode.dwFlags, KEYEVENTF_UNICODE);
    }
}
