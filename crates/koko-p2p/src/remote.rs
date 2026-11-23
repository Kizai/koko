use std::error::Error;
use std::time::Duration;

use iroh::{
    Endpoint, EndpointAddr, SecretKey,
    endpoint::{Connection, QuicTransportConfig, RecvStream, presets},
};
use koko_proto::{MAX_REMOTE_REPLY, REMOTE_ALPN, RemoteCommand, RemoteReply};
use tokio::time::timeout;

#[derive(Clone)]
pub struct RemoteClient {
    endpoint: Endpoint,
    connection: Connection,
}

impl RemoteClient {
    pub async fn connect(key: SecretKey, host: EndpointAddr) -> Result<Self, Box<dyn Error>> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .transport_config(
                QuicTransportConfig::builder()
                    .keep_alive_interval(Duration::from_secs(2))
                    .max_idle_timeout(Some(Duration::from_secs(8).try_into()?))
                    .build(),
            )
            .bind()
            .await?;
        let connection =
            timeout(Duration::from_secs(12), endpoint.connect(host, REMOTE_ALPN)).await??;
        Ok(Self {
            endpoint,
            connection,
        })
    }

    pub async fn send(&self, command: &RemoteCommand) -> Result<RemoteReply, Box<dyn Error>> {
        timeout(command.desktop_timeout() + Duration::from_secs(3), async {
            let mut recv = self.open_command(command).await?;
            Ok(serde_json::from_slice(
                &recv.read_to_end(MAX_REMOTE_REPLY).await?,
            )?)
        })
        .await?
    }

    pub async fn open_command(
        &self,
        command: &RemoteCommand,
    ) -> Result<RecvStream, Box<dyn Error>> {
        let (mut send, recv) = self.connection.open_bi().await?;
        send.write_all(&serde_json::to_vec(command)?).await?;
        send.finish()?;
        Ok(recv)
    }

    pub fn selected_path(&self) -> Option<(u64, bool)> {
        let paths = self.connection.paths();
        let path = paths.iter().find(|path| path.is_selected())?;
        let rtt = self.connection.rtt(path.id())?;
        Some((
            rtt.as_millis() as u64,
            matches!(path.remote_addr(), iroh::TransportAddr::Ip(_)),
        ))
    }

    pub async fn close(self) {
        self.connection.close(0u32.into(), b"command complete");
        self.endpoint.close().await;
    }
}

pub async fn send_command(
    key: SecretKey,
    host: EndpointAddr,
    command: &RemoteCommand,
) -> Result<RemoteReply, Box<dyn Error>> {
    let client = RemoteClient::connect(key, host).await?;
    let result = client.send(command).await;
    client.close().await;
    result
}
