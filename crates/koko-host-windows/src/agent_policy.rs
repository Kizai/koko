//! Windows agent approval rules. The desktop backend supplies the foreground
//! app and a hit test for each click before executing an unapproved agent input.

use koko_input::keyboard::{KeyboardSource, WindowsModifiers, mac_to_windows_scan};
use koko_proto::{KeyboardPlatform, PointerEvent, RemoteCommand};
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use std::{fmt, fs, io, path::Path};

#[derive(Debug, PartialEq, Eq)]
pub struct AgentPolicy {
    pub shortcuts: bool,
    pub command_apps: Vec<String>,
    pub protected_apps: Vec<String>,
    pub keywords: Vec<String>,
}

impl Default for AgentPolicy {
    fn default() -> Self {
        Self {
            shortcuts: true,
            command_apps: strings([
                "WindowsTerminal.exe",
                "cmd.exe",
                "powershell.exe",
                "pwsh.exe",
            ]),
            protected_apps: strings([
                "regedit.exe",
                "mmc.exe",
                "Taskmgr.exe",
                "SystemSettings.exe",
            ]),
            keywords: strings([
                "删除",
                "移除",
                "清空",
                "抹掉",
                "卸载",
                "付款",
                "支付",
                "购买",
                "转账",
                "发送",
                "提交订单",
                "永久",
                "delete",
                "remove",
                "erase",
                "empty trash",
                "uninstall",
                "pay",
                "purchase",
                "buy",
                "transfer",
                "send",
                "place order",
            ]),
        }
    }
}

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}

pub struct LoadedPolicy {
    pub policy: AgentPolicy,
    pub notice: Option<PolicyNotice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyNotice {
    InvalidFormat,
    Unreadable,
}

impl PolicyNotice {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidFormat => "agent-policy.json 格式有误，已改用默认的高危操作规则。",
            Self::Unreadable => "无法读取 agent-policy.json，已改用默认的高危操作规则。",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AppIdentity<'a> {
    /// Executable basename including its extension, e.g. `cmd.exe`.
    pub process_name: Option<&'a str>,
    pub aumid: Option<&'a str>,
    pub display_name: Option<&'a str>,
}

impl AppIdentity<'_> {
    fn matching_entry(self, entries: &[String]) -> Option<&str> {
        let process = self.process_name.map(str::to_lowercase);
        entries
            .iter()
            .find(|entry| {
                self.aumid == Some(entry.as_str())
                    || process
                        .as_ref()
                        .is_some_and(|name| *name == entry.to_lowercase())
            })
            .map(String::as_str)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PolicyHit {
    /// Stable identifier used for a session allowance.
    pub rule: String,
    pub summary: String,
}

impl<'de> Deserialize<'de> for AgentPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PolicyObject;

        impl<'de> Visitor<'de> for PolicyObject {
            type Value = AgentPolicy;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an agent policy object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut shortcuts = None;
                let mut command_apps = None;
                let mut protected_apps = None;
                let mut keywords = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "shortcuts" => read_first(&mut map, &mut shortcuts)?,
                        "command_apps" => read_first(&mut map, &mut command_apps)?,
                        "protected_apps" => read_first(&mut map, &mut protected_apps)?,
                        "keywords" => read_first(&mut map, &mut keywords)?,
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                let defaults = AgentPolicy::default();
                Ok(AgentPolicy {
                    shortcuts: shortcuts.flatten().unwrap_or(defaults.shortcuts),
                    command_apps: command_apps.flatten().unwrap_or(defaults.command_apps),
                    protected_apps: protected_apps.flatten().unwrap_or(defaults.protected_apps),
                    keywords: keywords.flatten().unwrap_or(defaults.keywords),
                })
            }
        }

        // JSONDecoder uses the first occurrence of each known key, even when its value is null.
        fn read_first<'de, M: MapAccess<'de>, T: Deserialize<'de>>(
            map: &mut M,
            slot: &mut Option<T>,
        ) -> Result<(), M::Error> {
            if slot.is_none() {
                *slot = Some(map.next_value()?);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
            Ok(())
        }

        deserializer.deserialize_map(PolicyObject)
    }
}

impl AgentPolicy {
    /// Omitted and null keys inherit defaults. A present list replaces its entire default list.
    pub fn parse(json: &[u8]) -> LoadedPolicy {
        let json = json.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(json);
        match serde_json::from_slice(json) {
            Ok(policy) => LoadedPolicy {
                policy,
                notice: None,
            },
            Err(_) => LoadedPolicy {
                policy: Self::default(),
                notice: Some(PolicyNotice::InvalidFormat),
            },
        }
    }

    /// Read the local configuration without changing the file or discarding the defaults on failure.
    pub fn load(path: &Path) -> LoadedPolicy {
        match fs::read(path) {
            Ok(json) => Self::parse(&json),
            Err(error) => LoadedPolicy {
                policy: Self::default(),
                notice: (error.kind() != io::ErrorKind::NotFound)
                    .then_some(PolicyNotice::Unreadable),
            },
        }
    }

    /// Checks an input batch in rule priority order. The hit test receives the
    /// display ID and protocol coordinates, returning UI Automation Name values
    /// in order from the clicked control to its nearest containers, excluding windows.
    pub fn check(
        &self,
        action: &RemoteCommand,
        frontmost: Option<AppIdentity<'_>>,
        cross_system_shortcuts: bool,
        mut control_names: impl FnMut(u32, u16, u16) -> Vec<String>,
    ) -> Option<PolicyHit> {
        let (events, source) = match action {
            RemoteCommand::Pointer {
                events, source_os, ..
            } => {
                let source = match source_os {
                    Some(KeyboardPlatform::Macos) => KeyboardSource::MacOs,
                    Some(KeyboardPlatform::Windows) => KeyboardSource::Windows,
                    _ => KeyboardSource::Other,
                };
                (events.as_slice(), source)
            }
            RemoteCommand::Text { .. } => (&[][..], KeyboardSource::Other),
            _ => return None,
        };
        let app_name = frontmost
            .and_then(|app| app.display_name.or(app.process_name).or(app.aumid))
            .unwrap_or("当前应用");

        if self.shortcuts {
            for event in events {
                if let PointerEvent::KeyDown { code, flags, .. } = *event
                    && let Some(name) = shortcut_name(code, flags, source, cross_system_shortcuts)
                {
                    return Some(PolicyHit {
                        rule: format!("shortcut:{name}"),
                        summary: format!("按 {name}（当前应用：{app_name}）"),
                    });
                }
            }
        }
        if let Some(app) = frontmost {
            if let Some(entry) = app.matching_entry(&self.protected_apps)
                && (matches!(action, RemoteCommand::Text { .. })
                    || events.iter().any(|event| {
                        matches!(
                            event,
                            PointerEvent::Down { .. } | PointerEvent::KeyDown { .. }
                        )
                    }))
            {
                return Some(PolicyHit {
                    rule: format!("app:{entry}"),
                    summary: format!("在「{app_name}」中点击或输入"),
                });
            }
            if let Some(entry) = app.matching_entry(&self.command_apps) {
                let summary = match action {
                    RemoteCommand::Text { text } => {
                        Some(format!("在「{app_name}」中输入：{}", preview(text)))
                    }
                    _ if events.iter().any(|event| {
                        matches!(
                            event,
                            PointerEvent::KeyDown {
                                code: 0x24 | 0x4c,
                                ..
                            }
                        )
                    }) =>
                    {
                        Some(format!("在「{app_name}」中按回车执行命令"))
                    }
                    _ => None,
                };
                if let Some(summary) = summary {
                    return Some(PolicyHit {
                        rule: format!("command:{entry}"),
                        summary,
                    });
                }
            }
        }
        if !self.keywords.is_empty()
            && let RemoteCommand::Pointer { display, .. } = *action
        {
            for event in events {
                if let PointerEvent::Down { x, y, .. } = *event {
                    let names = control_names(display, x, y);
                    if let Some(word) = self.matching_keyword(&names) {
                        return Some(PolicyHit {
                            rule: format!("click:{word}"),
                            summary: format!("点击「{}」", names[0]),
                        });
                    }
                }
            }
        }
        None
    }

    fn matching_keyword<'a>(&'a self, names: &[String]) -> Option<&'a str> {
        for name in names {
            let lower = name.to_lowercase();
            if let Some(word) = self.keywords.iter().find(|word| {
                // Swift's String.contains does not match an empty substring.
                !word.is_empty() && lower.contains(&word.to_lowercase())
            }) {
                return Some(word);
            }
        }
        None
    }
}

fn shortcut_name(
    code: u8,
    flags: u8,
    source: KeyboardSource,
    cross_system_shortcuts: bool,
) -> Option<&'static str> {
    let scan = mac_to_windows_scan(u16::from(code), source, cross_system_shortcuts)?;
    let WindowsModifiers {
        shift,
        ctrl,
        alt,
        win,
        ..
    } = WindowsModifiers::from_mac_flags(flags, source, cross_system_shortcuts);
    match (scan.make, scan.extended, shift, ctrl, alt, win) {
        (0x13, false, false, false, false, true) => Some("Win+R"),
        (0x3e, false, false, false, true, false) => Some("Alt+F4"),
        (0x01, false, true, true, false, false) => Some("Ctrl+Shift+Esc"),
        (0x53, true, true, false, false, false) => Some("Shift+Delete"),
        (0x2d, false, false, false, false, true) => Some("Win+X"),
        _ => None,
    }
}

fn preview(text: &str) -> String {
    let line = text.replace('\n', "⏎");
    let mut chars = line.chars();
    let mut preview: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}
