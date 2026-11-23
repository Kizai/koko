//! Read-only MCP server: lets an AI client pair with KoKo hosts and look at their screens.

use std::collections::{HashMap, hash_map::Entry};
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{EndpointId, SecretKey};
use koko_p2p::pairing::{PairingOptions, discover_hosts, request_agent_pairing, resolve_invite};
use koko_p2p::remote::RemoteClient;
use koko_proto::{ImageFormat, RemoteCommand, RemoteReply, Scope};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::registry::{HostRecord, Registry};

const DEFAULT_EDGE: u16 = 1280;
const JPEG_QUALITY: u8 = 80;
const INSTRUCTIONS: &str = "KoKo shows you the screens of Macs whose owners paired them with you. \
Call koko_list_devices first. If no device is paired, ask the user for the connection code shown \
in KoKo on the host Mac and call koko_pair; the host owner approves the first pairing. \
Use koko_screenshot to look at a display. Mouse and keyboard control are not available yet.";

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
                "This agent's access on the host does not allow that action.",
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

struct State {
    key: SecretKey,
    name: String,
    registry: Mutex<Registry>,
    clients: Mutex<HashMap<EndpointId, RemoteClient>>,
    screenshots: AtomicU64,
}

#[derive(Clone)]
pub struct KokoMcp {
    state: Arc<State>,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PairParams {
    /// The 10-character connection code shown in KoKo on the host Mac (for example ABCDE-FGHJK),
    /// or a direct invite that starts with koko://pair/v1/.
    pub code_or_invite: String,
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
            Entry::Occupied(entry) => entry.into_mut(),
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
                entry.insert(client)
            }
        };
        let result = client
            .send(command)
            .await
            .map_err(|error| format!("{error:?}"));
        if result.is_err()
            && let Some(client) = clients.remove(&id)
        {
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
        description = "Pair this agent with a KoKo host Mac using the connection code or direct invite the user reads from KoKo on that Mac. The host owner must approve the first pairing on the host; this call waits up to 90 seconds. Later connections to a trusted host need no approval."
    )]
    async fn koko_pair(
        &self,
        Parameters(params): Parameters<PairParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.pair(&params.code_or_invite).await)
    }

    #[tool(
        description = "Take a screenshot of a display on a paired KoKo host. Returns the image and JSON with screenshot_id, the display, the image size in pixels, and every display on the host.",
        annotations(read_only_hint = true)
    )]
    async fn koko_screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.screenshot(params).await)
    }
}

impl KokoMcp {
    async fn pair(&self, input: &str) -> Result<CallToolResult, Failure> {
        let (code, target) = resolve_invite(input)
            .await
            .map_err(|error| Failure::from_transport(&format!("{error:?}")))?;
        let id = target.id;
        let (address, scope) = request_agent_pairing(
            self.state.key.clone(),
            target.clone(),
            code.as_ref(),
            &self.state.name,
            Scope::Observer,
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
            name: format!("KoKo {}", id.fmt_short()),
            os: None,
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

    async fn screenshot(&self, params: ScreenshotParams) -> Result<CallToolResult, Failure> {
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
        let command = RemoteCommand::Capture {
            display: display.id,
            edge: params.max_edge.unwrap_or(DEFAULT_EDGE).clamp(320, 4096),
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
        Ok(CallToolResult::success(vec![
            ContentBlock::image(image, mime),
            ContentBlock::text(metadata.to_string()),
        ]))
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
    async fn offers_read_only_tools_and_reports_unpaired_devices() -> Result<(), Box<dyn Error>> {
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
        assert_eq!(names, ["koko_list_devices", "koko_pair", "koko_screenshot"]);

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
