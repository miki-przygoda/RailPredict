//! STOMP client for the Darwin Push Port firehose.
//!
//! ## Crate choice
//! No external STOMP crate is used. `stomp-rs` is unmaintained; `async-stomp` had
//! breaking API churn at time of writing. The STOMP protocol is simple enough
//! (text-framed over TCP) that a thin hand-rolled implementation over
//! `tokio::net::TcpStream` is more reliable than pinning to a dormant crate.
//!
//! ## STOMP frame format
//! ```text
//! COMMAND\n
//! header1:value1\n
//! header2:value2\n
//! \n
//! body\0
//! ```
//!
//! ## Darwin connection details
//! Read from environment variables at construction time:
//!   - `DARWIN_HOST`        — broker hostname (provided with Push Port credentials)
//!   - `DARWIN_PORT`        — broker port (default 61613)
//!   - `DARWIN_USERNAME`    — ActiveMQ username
//!   - `DARWIN_PASSWORD`    — ActiveMQ password
//!   - `DARWIN_DESTINATION` — STOMP topic (default `/topic/darwin.pushport-v16`)
//!
//! ## Reconnection
//! The `LiveStompClient::subscribe` method is a stream; the caller (ingestion pipeline)
//! is responsible for re-calling it on disconnect. The sequence guard in `filter.rs`
//! handles the message replay that Darwin issues on reconnect.

use async_trait::async_trait;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

#[allow(dead_code)]
const DEFAULT_DARWIN_PORT: u16 = 61613;
const DEFAULT_DESTINATION: &str = "/topic/darwin.pushport-v16";

/// A single STOMP frame received from the broker.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StompFrame {
    pub command: String,
    pub headers: Vec<(String, String)>,
    /// Raw body bytes (Darwin XML payload).
    pub body: Vec<u8>,
}

impl StompFrame {
    #[allow(dead_code)]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum StompError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("STOMP broker rejected connection: {0}")]
    ConnectionRejected(String),
    #[error("Unexpected frame command '{0}' (expected '{1}')")]
    UnexpectedCommand(String, String),
    #[error("Missing environment variable '{0}'")]
    MissingEnvVar(String),
    #[error("Connection closed by broker")]
    Disconnected,
}

/// Configuration for a Darwin STOMP connection.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct StompConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub destination: String,
}

impl StompConfig {
    pub fn from_env() -> Result<Self, StompError> {
        Ok(Self {
            host: env_var("DARWIN_HOST")?,
            port: std::env::var("DARWIN_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_DARWIN_PORT),
            username: env_var("DARWIN_USERNAME")?,
            password: env_var("DARWIN_PASSWORD")?,
            destination: std::env::var("DARWIN_DESTINATION")
                .unwrap_or_else(|_| DEFAULT_DESTINATION.to_string()),
        })
    }
}

fn env_var(name: &str) -> Result<String, StompError> {
    std::env::var(name).map_err(|_| StompError::MissingEnvVar(name.to_string()))
}

// ---------------------------------------------------------------------------
// Trait — testable interface
// ---------------------------------------------------------------------------

/// The interface the ingestion pipeline uses. `LiveStompClient` connects to Darwin;
/// `MockStompClient` injects pre-loaded frames for tests.
#[async_trait]
pub trait StompClient: Send + Sync {
    /// Connect, subscribe, and stream MESSAGE frames via the returned channel.
    /// Returns immediately; frames arrive asynchronously.
    async fn subscribe(
        &mut self,
        tx: mpsc::Sender<Result<StompFrame, StompError>>,
    ) -> Result<(), StompError>;
}

// ---------------------------------------------------------------------------
// Live implementation
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub struct LiveStompClient {
    config: StompConfig,
}

impl LiveStompClient {
    pub fn new(config: StompConfig) -> Self {
        Self { config }
    }

    pub fn from_env() -> Result<Self, StompError> {
        Ok(Self { config: StompConfig::from_env()? })
    }
}

#[async_trait]
impl StompClient for LiveStompClient {
    async fn subscribe(
        &mut self,
        tx: mpsc::Sender<Result<StompFrame, StompError>>,
    ) -> Result<(), StompError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let stream = TcpStream::connect(&addr).await?;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Send CONNECT frame.
        let connect = format!(
            "CONNECT\nlogin:{}\npasscode:{}\nheart-beat:0,0\n\n\0",
            self.config.username, self.config.password
        );
        writer.write_all(connect.as_bytes()).await?;

        // Read CONNECTED frame.
        let frame = read_frame(&mut reader).await?;
        if frame.command != "CONNECTED" {
            return Err(StompError::UnexpectedCommand(frame.command, "CONNECTED".into()));
        }

        // Send SUBSCRIBE frame.
        let subscribe = format!(
            "SUBSCRIBE\ndestination:{}\nid:sub-0\nack:auto\n\n\0",
            self.config.destination
        );
        writer.write_all(subscribe.as_bytes()).await?;

        // Stream MESSAGE frames to the caller.
        tokio::spawn(async move {
            loop {
                match read_frame(&mut reader).await {
                    Ok(frame) if frame.command == "MESSAGE" => {
                        if tx.send(Ok(frame)).await.is_err() {
                            break; // receiver dropped
                        }
                    }
                    Ok(_) => {} // ignore non-MESSAGE frames (HEARTBEAT etc.)
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}

/// Read one STOMP frame from a buffered reader.
#[allow(dead_code)]
async fn read_frame(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<StompFrame, StompError> {
    let mut command = String::new();
    // Skip blank lines (heart-beat frames are just "\n").
    while command.trim().is_empty() {
        command.clear();
        let n = reader.read_line(&mut command).await?;
        if n == 0 {
            return Err(StompError::Disconnected);
        }
    }

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.to_string(), v.to_string()));
        }
    }

    // Read body up to the NULL terminator.
    let mut body = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = {
            use tokio::io::AsyncReadExt;
            reader.read(&mut byte).await?
        };
        if n == 0 {
            return Err(StompError::Disconnected);
        }
        if byte[0] == 0 {
            break;
        }
        body.push(byte[0]);
    }

    Ok(StompFrame { command: command.trim().to_string(), headers, body })
}

// ---------------------------------------------------------------------------
// Mock client
// ---------------------------------------------------------------------------

/// Feeds pre-loaded frames to the pipeline without a network connection.
pub struct MockStompClient {
    frames: Vec<Result<StompFrame, StompError>>,
}

impl MockStompClient {
    #[allow(dead_code)]
    pub fn new(frames: Vec<Result<StompFrame, StompError>>) -> Self {
        Self { frames }
    }

    pub fn with_xml_payloads(payloads: Vec<String>) -> Self {
        let frames = payloads
            .into_iter()
            .map(|xml| {
                Ok(StompFrame {
                    command: "MESSAGE".to_string(),
                    headers: vec![("destination".to_string(), DEFAULT_DESTINATION.to_string())],
                    body: xml.into_bytes(),
                })
            })
            .collect();
        Self { frames }
    }
}

#[async_trait]
impl StompClient for MockStompClient {
    async fn subscribe(
        &mut self,
        tx: mpsc::Sender<Result<StompFrame, StompError>>,
    ) -> Result<(), StompError> {
        let frames = std::mem::take(&mut self.frames);
        tokio::spawn(async move {
            for frame in frames {
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_delivers_frames() {
        let xml = r#"<Pport ts="2024-04-17T12:00:00Z" version="16.0"><uR><TS rid="202404170123456" ssd="2024-04-17" uid="C12345"><Location tpl="LEEDS" ptd="12:00"/></TS></uR></Pport>"#;
        let mut client = MockStompClient::with_xml_payloads(vec![xml.to_string()]);
        let (tx, mut rx) = mpsc::channel(10);
        client.subscribe(tx).await.unwrap();

        let frame = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            rx.recv(),
        )
        .await
        .expect("timed out")
        .expect("channel closed")
        .expect("frame error");

        assert_eq!(frame.command, "MESSAGE");
        assert!(!frame.body.is_empty());
    }

    #[tokio::test]
    async fn mock_delivers_multiple_frames_in_order() {
        let payloads: Vec<_> = (0..3).map(|i| format!("<msg id=\"{i}\"/>")).collect();
        let mut client = MockStompClient::with_xml_payloads(payloads.clone());
        let (tx, mut rx) = mpsc::channel(10);
        client.subscribe(tx).await.unwrap();

        let mut received = Vec::new();
        for _ in 0..3 {
            let frame = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                rx.recv(),
            )
            .await
            .unwrap()
            .unwrap()
            .unwrap();
            received.push(String::from_utf8(frame.body).unwrap());
        }
        assert_eq!(received, payloads);
    }
}
