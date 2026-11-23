//! MCP server: lets an AI client pair with KoKo hosts, look at their screens and operate them.

use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{EndpointId, SecretKey};
use koko_p2p::pairing::{PairingOptions, discover_hosts, request_agent_pairing, resolve_invite};
use koko_p2p::remote::RemoteClient;
use koko_proto::{
    HELP_TIMEOUT_SECONDS, HumanResult, ImageFormat, MAX_HELP_REASON, MouseButton, RemoteCommand,
    RemoteReply, Scope,
};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::input::{Gesture, key_events, mouse_events, to_protocol};
use crate::registry::{HostRecord, Registry};

const DEFAULT_EDGE: u16 = 1280;
const JPEG_QUALITY: u8 = 80;
const MAX_SHOTS: usize = 64;
const SETTLE: Duration = Duration::from_millis(600);
const INSTRUCTIONS: &str = "KoKo lets you see and operate Macs whose owners paired them with you. \
Call koko_list_devices first. If no device is paired, ask the user for the connection code shown \
in KoKo on the host Mac and call koko_pair; the host owner approves the first pairing. \
Take a screenshot with koko_screenshot, then use koko_mouse and koko_keyboard with pixel coordinates \
from that screenshot. The host owner sees when you are in control and can stop you at any time. \
If a result says preempted_by_human, stop and tell the user; do not retry until they ask. \
Call koko_request_human when a step needs a person, such as signing in or confirming a payment. \
Call koko_release_control when you finish.";

/// A tool outcome the model should act on, reported as a tool error rather than a protocol error.
struct Failure {
    status: &'static str,
    message: String,
}

impl Failure {
    fn new(status: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// Maps a host error code from `koko/remote/1` to guidance for the model.
    fn from_host(code: &str) -> Self {
        match code {
            "scope_denied" => Self::new(
                "scope_denied",
                "This agent may only view this host. To control it, call koko_pair with this device_id and scope agent_safe; the host owner must approve.",
            ),
            "preempted_by_human" => Self::new(
                "preempted_by_human",
                "A person on the host took over or pressed stop. Stop and tell the user. Do not retry until they ask you to continue.",
            ),
            "input_permission_required" => Self::new(
                "input_permission_required",
                "The host Mac has not allowed KoKo to control the mouse and keyboard. Ask the user to grant Accessibility on the host.",
            ),
            "input_failed" => Self::new("input_failed", "The host could not send that input."),
            "awaiting_human" => Self::new(
                "awaiting_human",
                "A request for help is still open on this host. Wait for its answer before acting.",
            ),
            "screen_permission_required" => Self::new(
                "screen_permission_required",
                "The host Mac has not allowed KoKo to record the screen. Ask the user to grant Screen Recording on the host.",
            ),
            "display_unavailable" => Self::new(
                "display_unavailable",
                "That display is not available. Take a screenshot without `display` to use the main display.",
            ),
            "capture_busy" => {
                Self::new("capture_busy", "The host is busy capturing. Retry shortly.")
            }
            "capture_failed" => Self::new(
                "capture_failed",
                "The host could not capture its screen. It may be locked or asleep.",
            ),
            "desktop_unavailable" => Self::new(
                "desktop_unavailable",
                "KoKo on the host is not ready. Ask the user to check that sharing is on.",
            ),
            "invalid_command" => Self::new(
                "invalid_command",
                "The host runs an older KoKo. Ask the user to update KoKo on both Macs.",
            ),
            other => Self::new("host_error", format!("The host returned `{other}`.")),
        }
    }

    /// Maps connection and pairing errors, which arrive as text from iroh and koko-p2p.
    fn from_transport(error: &str) -> Self {
        if error.contains("agent_busy") {
            Self::new(
                "agent_busy",
                "Another agent is already connected to this host. Tell the user.",
            )
        } else if error.contains("pairing declined or expired") {
            Self::new(
                "declined",
                "The host owner declined the request, or did not answer in time.",
            )
        } else if error.contains("invalid pairing code") || error.contains("connection code") {
            Self::new(
                "invalid_code",
                "The connection code is not valid. Ask the user for the current code shown on the host.",
            )
        } else if error.contains("host not found") {
            Self::new(
                "host_not_found",
                "No KoKo host with this code was found on the local network. Ask the user for the direct invite instead.",
            )
        } else if error.contains("peer not authorized") {
            Self::new(
                "not_paired",
                "The host no longer accepts this agent. Ask the user for a new connection code and call koko_pair.",
            )
        } else {
            Self::new(
                "host_unreachable",
                format!("Could not reach the host: {error}"),
            )
        }
    }

    fn into_result(self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text(
            json!({"status": self.status, "message": self.message}).to_string(),
        )])
    }
}

fn finish(result: Result<CallToolResult, Failure>) -> Result<CallToolResult, McpError> {
    Ok(result.unwrap_or_else(Failure::into_result))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_secs())
}

/// Where a screenshot came from and how big it was, so its pixel coordinates can be mapped back.
#[derive(Clone)]
struct Shot {
    id: String,
    device: EndpointId,
    display: u32,
    width: u32,
    height: u32,
    edge: u16,
}

struct State {
    key: SecretKey,
    name: String,
    registry: Mutex<Registry>,
    clients: Mutex<HashMap<EndpointId, RemoteClient>>,
    screenshots: AtomicU64,
    shots: std::sync::Mutex<VecDeque<Shot>>,
}

#[derive(Clone)]
pub struct KokoMcp {
    state: Arc<State>,
    tool_router: ToolRouter<Self>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentScope {
    Observer,
    AgentSafe,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PairParams {
    /// The 10-character connection code shown in KoKo on the host Mac (for example ABCDE-FGHJK),
    /// or a direct invite that starts with koko://pair/v1/. Omit it to ask an already paired
    /// device again by device_id, for example for more access.
    pub code_or_invite: Option<String>,
    /// Device ID of an already paired host. Used when code_or_invite is omitted.
    pub device_id: Option<String>,
    /// "agent_safe" (default) to view and control the Mac, or "observer" to only view it.
    pub scope: Option<AgentScope>,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    Move,
    Click,
    DoubleClick,
    RightClick,
    MiddleClick,
    Drag,
    Scroll,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MouseParams {
    /// Device ID from koko_list_devices.
    pub device_id: String,
    pub action: MouseAction,
    /// X in pixels of the screenshot, from the left edge.
    pub x: u32,
    /// Y in pixels of the screenshot, from the top edge.
    pub y: u32,
    /// Drag end X in screenshot pixels. Required for drag.
    pub to_x: Option<u32>,
    /// Drag end Y in screenshot pixels. Required for drag.
    pub to_y: Option<u32>,
    /// Horizontal scroll in pixels; positive scrolls toward the left of the content.
    pub dx: Option<i32>,
    /// Vertical scroll in pixels; positive scrolls toward the top of the content.
    pub dy: Option<i32>,
    /// Screenshot the coordinates refer to. Defaults to the latest screenshot of this device.
    pub screenshot_id: Option<String>,
    /// Return a new screenshot of the same display after the action settles.
    pub screenshot_after: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct KeyboardParams {
    /// Device ID from koko_list_devices.
    pub device_id: String,
    /// Text to type into the focused field, up to 2048 bytes. Use this for literal text.
    pub text: Option<String>,
    /// Key combos pressed in order, such as ["cmd+a", "delete"] or ["cmd+shift+t"]. Modifiers:
    /// cmd, ctrl, opt, shift, fn. Keys: a-z, 0-9, enter, escape, tab, space, delete, forward_delete,
    /// up, down, left, right, home, end, page_up, page_down, f1-f12, and ANSI punctuation.
    pub keys: Option<Vec<String>>,
    /// Return a new screenshot after the keys settle.
    pub screenshot_after: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct HelpParams {
    /// Device ID from koko_list_devices.
    pub device_id: String,
    /// What you need the person on the host Mac to do, in one or two sentences, up to 500 characters.
    pub reason: String,
    /// How long to wait for an answer, from 10 to 600 seconds. Defaults to 300.
    pub timeout_s: Option<u16>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct DeviceParams {
    /// Device ID from koko_list_devices.
    pub device_id: String,
}

#[derive(Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotFormat {
    Jpeg,
    Png,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ScreenshotParams {
    /// Device ID from koko_list_devices.
    pub device_id: String,
    /// Display ID from a previous screenshot result. Defaults to the main display.
    pub display: Option<u32>,
    /// Longest image edge in pixels, from 320 to 4096. Defaults to 1280.
    pub max_edge: Option<u16>,
    /// "jpeg" (default, smaller) or "png" (lossless, larger).
    pub format: Option<ScreenshotFormat>,
    /// Whether to draw the mouse pointer. Defaults to true.
    pub cursor: Option<bool>,
}

impl KokoMcp {
    pub fn new(key: SecretKey, name: String, registry: Registry) -> Self {
        Self {
            state: Arc::new(State {
                key,
                name,
                registry: Mutex::new(registry),
                clients: Mutex::new(HashMap::new()),
                screenshots: AtomicU64::new(0),
                shots: std::sync::Mutex::new(VecDeque::new()),
            }),
            tool_router: Self::tool_router(),
        }
    }

    fn options() -> PairingOptions {
        PairingOptions {
            prefer_lan: true,
            enhanced: true,
        }
    }

    async fn record(&self, id: EndpointId) -> Result<HostRecord, Failure> {
        self.state.registry.lock().await.get(id).cloned().ok_or_else(|| {
            Failure::new(
                "not_paired",
                "This device is not paired. Call koko_list_devices, or koko_pair with a code from the user.",
            )
        })
    }

    async fn save(&self, record: HostRecord) -> Result<(), Failure> {
        self.state
            .registry
            .lock()
            .await
            .upsert(record)
            .map_err(|error| {
                Failure::new(
                    "storage_failed",
                    format!("Could not save the device list: {error}"),
                )
            })
    }

    /// Sends one command on the device's long-lived connection. A failed connection is dropped.
    async fn send_once(
        &self,
        id: EndpointId,
        command: &RemoteCommand,
    ) -> Result<RemoteReply, String> {
        let mut clients = self.state.clients.lock().await;
        let client = match clients.entry(id) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let target = self
                    .state
                    .registry
                    .lock()
                    .await
                    .get(id)
                    .map(HostRecord::target)
                    .ok_or("not paired")?;
                let client = RemoteClient::connect(self.state.key.clone(), target)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                entry.insert(client).clone()
            }
        };
        // Requests share the connection on separate streams; a request for help may wait minutes.
        drop(clients);
        let result = client
            .send(command)
            .await
            .map_err(|error| format!("{error:?}"));
        if result.is_err() {
            self.state.clients.lock().await.remove(&id);
            client.close().await;
        }
        result
    }

    /// Sends a command, reconnecting once and then pairing again by trust if the host restarted.
    async fn request(
        &self,
        id: EndpointId,
        command: &RemoteCommand,
    ) -> Result<RemoteReply, Failure> {
        let error = match self.send_once(id, command).await {
            Ok(reply) => return Ok(reply),
            Err(_) => match self.send_once(id, command).await {
                Ok(reply) => return Ok(reply),
                Err(error) => error,
            },
        };
        if !error.contains("peer not authorized") {
            self.rediscover(id).await?;
        }
        self.repair(id).await?;
        self.send_once(id, command)
            .await
            .map_err(|error| Failure::from_transport(&error))
    }

    /// Refreshes the host's addresses from LAN discovery after a failed connection.
    async fn rediscover(&self, id: EndpointId) -> Result<(), Failure> {
        let found = discover_hosts().await.unwrap_or_default();
        let Some(host) = found.into_iter().find(|host| host.target.id == id) else {
            return Ok(());
        };
        let mut record = self.record(id).await?;
        record.addresses = host.target.ip_addrs().copied().collect();
        record.os = host.os.or(record.os);
        self.save(record).await
    }

    /// Asks the host for a session again. A trusted agent is let in without a prompt.
    async fn repair(&self, id: EndpointId) -> Result<(), Failure> {
        let mut record = self.record(id).await?;
        let (address, scope) = request_agent_pairing(
            self.state.key.clone(),
            record.target(),
            None,
            &self.state.name,
            record.scope,
            Self::options(),
        )
        .await
        .map_err(|error| Failure::from_transport(&format!("{error:?}")))?;
        record.addresses.retain(|existing| *existing != address);
        record.addresses.insert(0, address);
        record.scope = scope;
        self.save(record).await
    }
}

fn invalid(message: impl Into<String>) -> Failure {
    Failure::new("invalid_input", message)
}

fn device_id(value: &str) -> Result<EndpointId, Failure> {
    value.trim().parse().map_err(|_| {
        Failure::new(
            "unknown_device",
            "That is not a KoKo device ID. Use an ID from koko_list_devices.",
        )
    })
}

#[tool_router]
impl KokoMcp {
    #[tool(
        description = "List the KoKo host Macs this agent has paired with, whether each is reachable on the local network now, and the access each granted.",
        annotations(read_only_hint = true)
    )]
    async fn koko_list_devices(&self) -> Result<CallToolResult, McpError> {
        let discovered = discover_hosts().await.unwrap_or_default();
        let connected: Vec<EndpointId> = self.state.clients.lock().await.keys().copied().collect();
        let mut registry = self.state.registry.lock().await;
        let mut changed = false;
        for host in &mut registry.hosts {
            if let Some(found) = discovered.iter().find(|found| found.target.id == host.id)
                && found.os.is_some()
                && found.os != host.os
            {
                host.os = found.os.clone();
                changed = true;
            }
        }
        if changed && let Err(error) = registry.save() {
            eprintln!("Could not save the device list: {error}");
        }
        let devices: Vec<_> = registry
            .hosts
            .iter()
            .map(|host| {
                json!({
                    "device_id": host.id.to_string(),
                    "name": host.name,
                    "os": host.os,
                    "online": connected.contains(&host.id)
                        || discovered.iter().any(|found| found.target.id == host.id),
                    "scope": host.scope,
                })
            })
            .collect();
        let mut result = json!({"devices": devices});
        if devices.is_empty() {
            result["message"] = "No paired devices. Ask the user for the KoKo connection code shown on the host Mac, then call koko_pair.".into();
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            result.to_string(),
        )]))
    }

    #[tool(
        description = "Pair this agent with a KoKo host Mac using the connection code or direct invite the user reads from KoKo on that Mac, or ask an already paired device again by device_id. The host owner must approve the first pairing and any request for more access; this call waits up to 90 seconds. Later connections to a trusted host need no approval."
    )]
    async fn koko_pair(
        &self,
        Parameters(params): Parameters<PairParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.pair(params).await)
    }

    #[tool(
        description = "Take a screenshot of a display on a paired KoKo host. Returns the image and JSON with screenshot_id, the display, the image size in pixels, and every display on the host.",
        annotations(read_only_hint = true)
    )]
    async fn koko_screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.screenshot(params).await.map(CallToolResult::success))
    }

    #[tool(
        description = "Move, click, double-click, right-click, drag or scroll on a paired KoKo host. Coordinates are pixels in a screenshot from koko_screenshot. Your first input takes control of the host; a person's input takes it back."
    )]
    async fn koko_mouse(
        &self,
        Parameters(params): Parameters<MouseParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.mouse(params).await)
    }

    #[tool(
        description = "Type text or press key combos on a paired KoKo host. Input goes to the focused app. Use text for literal text and keys for shortcuts and special keys."
    )]
    async fn koko_keyboard(
        &self,
        Parameters(params): Parameters<KeyboardParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.keyboard(params).await)
    }

    #[tool(
        description = "Ask the person at a KoKo host Mac to do something only a person should do, such as signing in, solving a CAPTCHA, approving a payment or confirming a risky step. Blocks until they answer or the timeout passes. Results: resolved (they did it; take a new screenshot and continue), declined or taken_over (stop and tell the user), timeout (nobody answered; you hold control again)."
    )]
    async fn koko_request_human(
        &self,
        Parameters(params): Parameters<HelpParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.request_help(params).await)
    }

    #[tool(
        description = "Give control of a KoKo host back to its owner when you are done operating it."
    )]
    async fn koko_release_control(
        &self,
        Parameters(params): Parameters<DeviceParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.release(&params.device_id).await)
    }
}

impl KokoMcp {
    async fn pair(&self, params: PairParams) -> Result<CallToolResult, Failure> {
        let scope = match params.scope.unwrap_or(AgentScope::AgentSafe) {
            AgentScope::Observer => Scope::Observer,
            AgentScope::AgentSafe => Scope::AgentSafe,
        };
        let (code, target, previous) = match (params.code_or_invite, params.device_id) {
            (Some(input), _) => {
                let (code, target) = resolve_invite(&input)
                    .await
                    .map_err(|error| Failure::from_transport(&format!("{error:?}")))?;
                (code, target, None)
            }
            (None, Some(device)) => {
                let record = self.record(device_id(&device)?).await?;
                (None, record.target(), Some(record))
            }
            (None, None) => {
                return Err(invalid(
                    "Give code_or_invite, or device_id of a paired host.",
                ));
            }
        };
        let id = target.id;
        let (address, scope) = request_agent_pairing(
            self.state.key.clone(),
            target.clone(),
            code.as_ref(),
            &self.state.name,
            scope,
            Self::options(),
        )
        .await
        .map_err(|error| Failure::from_transport(&format!("{error:?}")))?;

        let mut addresses = vec![address];
        addresses.extend(target.ip_addrs().copied().filter(|other| *other != address));
        if let Some(client) = self.state.clients.lock().await.remove(&id) {
            client.close().await;
        }
        let mut record = HostRecord {
            id,
            name: previous.as_ref().map_or_else(
                || format!("KoKo {}", id.fmt_short()),
                |record| record.name.clone(),
            ),
            os: previous.and_then(|record| record.os),
            addresses,
            scope,
            paired_at: now(),
        };
        self.save(record.clone()).await?;
        if let Ok(RemoteReply::Displays { host_name, .. }) =
            self.request(id, &RemoteCommand::Displays).await
            && !host_name.trim().is_empty()
        {
            record.name = host_name;
            self.save(record.clone()).await?;
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(
            json!({
                "status": "paired",
                "device_id": id.to_string(),
                "name": record.name,
                "scope": scope,
            })
            .to_string(),
        )]))
    }

    async fn screenshot(&self, params: ScreenshotParams) -> Result<Vec<ContentBlock>, Failure> {
        let id = device_id(&params.device_id)?;
        self.record(id).await?;
        let displays = match self.request(id, &RemoteCommand::Displays).await? {
            RemoteReply::Displays { displays, .. } => displays,
            RemoteReply::Error { code } => return Err(Failure::from_host(&code)),
            _ => return Err(Failure::from_host("invalid_command")),
        };
        let display = match params.display {
            Some(wanted) => displays.iter().find(|display| display.id == wanted),
            None => displays.first(),
        }
        .ok_or_else(|| Failure::from_host("display_unavailable"))?;
        let requested = match params.format.unwrap_or(ScreenshotFormat::Jpeg) {
            ScreenshotFormat::Jpeg => ImageFormat::Jpeg,
            ScreenshotFormat::Png => ImageFormat::Png,
        };
        let edge = params.max_edge.unwrap_or(DEFAULT_EDGE).clamp(320, 4096);
        let command = RemoteCommand::Capture {
            display: display.id,
            edge,
            quality: JPEG_QUALITY,
            format: Some(requested),
            cursor: Some(params.cursor.unwrap_or(true)),
        };
        let (image, width, height, format) = match self.request(id, &command).await? {
            RemoteReply::Image {
                image,
                width: Some(width),
                height: Some(height),
                format,
            } => (image, width, height, format.unwrap_or(requested)),
            RemoteReply::Error { code } => return Err(Failure::from_host(&code)),
            // Hosts before agent support ignore the format and omit the image size.
            _ => return Err(Failure::from_host("invalid_command")),
        };
        let screenshot_id = format!(
            "s{}",
            self.state.screenshots.fetch_add(1, Ordering::Relaxed) + 1
        );
        {
            let mut shots = self.state.shots.lock().unwrap();
            if shots.len() == MAX_SHOTS {
                shots.pop_front();
            }
            shots.push_back(Shot {
                id: screenshot_id.clone(),
                device: id,
                display: display.id,
                width,
                height,
                edge,
            });
        }
        let main = displays.first().map(|first| first.id);
        let metadata = json!({
            "screenshot_id": screenshot_id,
            "display": display,
            "width": width,
            "height": height,
            "displays": displays
                .iter()
                .map(|display| json!({
                    "id": display.id,
                    "name": display.name,
                    "width": display.width,
                    "height": display.height,
                    "main": Some(display.id) == main,
                }))
                .collect::<Vec<_>>(),
        });
        let mime = match format {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
        };
        Ok(vec![
            ContentBlock::image(image, mime),
            ContentBlock::text(metadata.to_string()),
        ])
    }

    fn shot(&self, device: EndpointId, id: Option<&str>) -> Result<Shot, Failure> {
        let shots = self.state.shots.lock().unwrap();
        let shot = match id {
            Some(id) => shots
                .iter()
                .rev()
                .find(|shot| shot.id == id && shot.device == device),
            None => shots.iter().rev().find(|shot| shot.device == device),
        };
        shot.cloned().ok_or_else(|| {
            Failure::new(
                "screenshot_required",
                "Take a screenshot of this device with koko_screenshot first, then use its pixel coordinates.",
            )
        })
    }

    /// Sends one input command and, if asked, returns a fresh screenshot once the screen settles.
    async fn act(
        &self,
        id: EndpointId,
        command: RemoteCommand,
        display: u32,
        edge: u16,
        screenshot_after: Option<bool>,
    ) -> Result<CallToolResult, Failure> {
        match self.request(id, &command).await? {
            RemoteReply::Ok => {}
            RemoteReply::Error { code } => return Err(Failure::from_host(&code)),
            _ => return Err(Failure::from_host("invalid_command")),
        }
        let mut content = vec![ContentBlock::text(json!({"status": "ok"}).to_string())];
        if screenshot_after.unwrap_or(false) {
            tokio::time::sleep(SETTLE).await;
            content.extend(
                self.screenshot(ScreenshotParams {
                    device_id: id.to_string(),
                    display: Some(display),
                    max_edge: Some(edge),
                    format: None,
                    cursor: None,
                })
                .await?,
            );
        }
        Ok(CallToolResult::success(content))
    }

    async fn mouse(&self, params: MouseParams) -> Result<CallToolResult, Failure> {
        let id = device_id(&params.device_id)?;
        let shot = self.shot(id, params.screenshot_id.as_deref())?;
        let point = |x: u32, y: u32| {
            to_protocol(x, shot.width)
                .zip(to_protocol(y, shot.height))
                .ok_or_else(|| {
                    Failure::new(
                        "out_of_bounds",
                        format!(
                            "({x}, {y}) is outside the {}x{} screenshot.",
                            shot.width, shot.height
                        ),
                    )
                })
        };
        let gesture = match params.action {
            MouseAction::Move => Gesture::Move,
            MouseAction::Click => Gesture::Click(MouseButton::Left),
            MouseAction::RightClick => Gesture::Click(MouseButton::Right),
            MouseAction::MiddleClick => Gesture::Click(MouseButton::Middle),
            MouseAction::DoubleClick => Gesture::DoubleClick,
            MouseAction::Drag => {
                let (Some(x), Some(y)) = (params.to_x, params.to_y) else {
                    return Err(invalid("drag needs to_x and to_y."));
                };
                Gesture::Drag { to: point(x, y)? }
            }
            MouseAction::Scroll => {
                let clamp = |value: Option<i32>| {
                    value
                        .unwrap_or(0)
                        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
                };
                let (dx, dy) = (clamp(params.dx), clamp(params.dy));
                if dx == 0 && dy == 0 {
                    return Err(invalid("scroll needs a non-zero dx or dy."));
                }
                Gesture::Scroll { dx, dy }
            }
        };
        let command = RemoteCommand::Pointer {
            display: shot.display,
            events: mouse_events(point(params.x, params.y)?, gesture),
            source_os: None,
        };
        self.act(
            id,
            command,
            shot.display,
            shot.edge,
            params.screenshot_after,
        )
        .await
    }

    async fn keyboard(&self, params: KeyboardParams) -> Result<CallToolResult, Failure> {
        let id = device_id(&params.device_id)?;
        self.record(id).await?;
        let shot = self.shot(id, None).ok();
        // Keys go to the focused app; the display only has to exist on the host.
        let display = match &shot {
            Some(shot) => shot.display,
            None => match self.request(id, &RemoteCommand::Displays).await? {
                RemoteReply::Displays { displays, .. } => displays
                    .first()
                    .map(|display| display.id)
                    .ok_or_else(|| Failure::from_host("display_unavailable"))?,
                RemoteReply::Error { code } => return Err(Failure::from_host(&code)),
                _ => return Err(Failure::from_host("invalid_command")),
            },
        };
        let command = match (params.text, params.keys) {
            (Some(text), None) => {
                if text.is_empty() || text.len() > 2048 || text.contains('\0') {
                    return Err(invalid("text must be 1 to 2048 bytes without NUL."));
                }
                RemoteCommand::Text { text }
            }
            (None, Some(keys)) => RemoteCommand::Pointer {
                display,
                events: key_events(&keys).map_err(invalid)?,
                source_os: None,
            },
            _ => return Err(invalid("Give exactly one of text or keys.")),
        };
        let edge = shot.map_or(DEFAULT_EDGE, |shot| shot.edge);
        self.act(id, command, display, edge, params.screenshot_after)
            .await
    }

    async fn request_help(&self, params: HelpParams) -> Result<CallToolResult, Failure> {
        let id = device_id(&params.device_id)?;
        self.record(id).await?;
        let reason = params.reason.trim().to_owned();
        let timeout_s = params.timeout_s.unwrap_or(300);
        if reason.is_empty()
            || reason.chars().count() > MAX_HELP_REASON
            || !HELP_TIMEOUT_SECONDS.contains(&timeout_s)
        {
            return Err(invalid(
                "reason must be 1 to 500 characters and timeout_s 10 to 600 seconds.",
            ));
        }
        let (result, note) = match self
            .request(id, &RemoteCommand::RequestHuman { reason, timeout_s })
            .await?
        {
            RemoteReply::Human { result, note } => (result, note),
            RemoteReply::Error { code } => return Err(Failure::from_host(&code)),
            _ => return Err(Failure::from_host("invalid_command")),
        };
        let message = match result {
            HumanResult::Resolved => {
                "The person finished. Take a new screenshot before you continue."
            }
            HumanResult::Declined => "The person declined. Stop this task and tell the user.",
            HumanResult::TakenOver => {
                "The person took over. Stop and wait until the user asks you to continue."
            }
            HumanResult::Timeout => {
                "Nobody answered in time. You hold control again; decide whether to continue or stop."
            }
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(
            json!({"status": result, "note": note, "message": message}).to_string(),
        )]))
    }

    async fn release(&self, device: &str) -> Result<CallToolResult, Failure> {
        let id = device_id(device)?;
        self.record(id).await?;
        match self.request(id, &RemoteCommand::ReleaseControl).await? {
            RemoteReply::Ok => Ok(CallToolResult::success(vec![ContentBlock::text(
                json!({"status": "released"}).to_string(),
            )])),
            RemoteReply::Error { code } => Err(Failure::from_host(&code)),
            _ => Err(Failure::from_host("invalid_command")),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KokoMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("koko", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serves MCP on stdio. Diagnostics go to stderr because stdout carries the protocol.
pub async fn serve(key: SecretKey, identity: &Path, name: String) -> Result<(), Box<dyn Error>> {
    let registry = Registry::load(Registry::path_for(identity))?;
    eprintln!("KoKo MCP agent {} ({name})", key.public());
    let service = KokoMcp::new(key, name, registry)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rmcp::model::CallToolRequestParams;

    use super::*;

    #[tokio::test]
    async fn offers_the_tools_and_reports_unpaired_devices() -> Result<(), Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let registry = Registry::load(directory.path().join("agent-hosts.json"))?;
        let (server_io, client_io) = tokio::io::duplex(64 * 1024);
        let server = KokoMcp::new(SecretKey::generate(), "Test Agent".into(), registry);
        let server = tokio::spawn(async move {
            let running = server.serve(server_io).await?;
            running.waiting().await?;
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        });
        let client = ().serve(client_io).await?;

        let mut names: Vec<_> = client
            .list_all_tools()
            .await?
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "koko_keyboard",
                "koko_list_devices",
                "koko_mouse",
                "koko_pair",
                "koko_release_control",
                "koko_request_human",
                "koko_screenshot"
            ]
        );

        let device = SecretKey::generate().public().to_string();
        let result = client
            .call_tool(
                CallToolRequestParams::new("koko_screenshot")
                    .with_arguments(json!({"device_id": device}).as_object().cloned().unwrap()),
            )
            .await?;
        assert_eq!(result.is_error, Some(true));
        let text = result.content[0].as_text().unwrap().text.clone();
        assert!(text.contains("not_paired"), "{text}");

        client.cancel().await?;
        let _ = server.await;
        Ok(())
    }
}
