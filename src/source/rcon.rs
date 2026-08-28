//! A small wrapper over the RCON connection that reconnects on its own.
//!
//! The server closes the connection when it is restarted or when the tick
//! thread stalls long enough to time the socket out, which is exactly the
//! moment an operator is watching. So a dropped link is treated as ordinary:
//! the state becomes [`Link::Down`] with the reason, and the next command
//! attempts a reconnect.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use rcon::Connection;
use tokio::net::TcpStream;
use tokio::time;

use crate::config::RconConfig;
use crate::metrics::Link;
use crate::server::Address;

/// Port RCON listens on by default.
pub const DEFAULT_RCON_PORT: u16 = 25575;

/// An RCON connection that re-establishes itself as needed.
pub struct RconClient {
    address: Address,
    password: String,
    connect_timeout: Duration,
    command_timeout: Duration,
    reconnect_delay: Duration,
    connection: Option<Connection<TcpStream>>,
    /// When the last failure happened, so retries are not hammered out.
    last_failure: Option<Instant>,
    link: Link,
}

impl RconClient {
    /// Build a client from the config. Does not connect yet.
    pub fn new(config: &RconConfig) -> anyhow::Result<Self> {
        let address = Address::parse_with_default_port(&config.address, DEFAULT_RCON_PORT)
            .context("parsing rcon.address")?;
        let password = config.resolve_password()?;

        Ok(Self {
            address,
            password,
            connect_timeout: Duration::from_millis(config.connect_timeout_ms.max(250)),
            command_timeout: Duration::from_millis(config.command_timeout_ms.max(250)),
            reconnect_delay: Duration::from_millis(config.reconnect_delay_ms.max(250)),
            connection: None,
            last_failure: None,
            link: Link::Connecting,
        })
    }

    pub fn address(&self) -> &Address {
        &self.address
    }

    pub fn link(&self) -> &Link {
        &self.link
    }

    /// Run a console command, connecting first if necessary.
    ///
    /// After a failure, further attempts are refused until the reconnect delay
    /// has passed, so a downed server does not turn into a busy loop.
    pub async fn command(&mut self, command: &str) -> anyhow::Result<String> {
        if self.connection.is_none() {
            if let Some(failed_at) = self.last_failure
                && failed_at.elapsed() < self.reconnect_delay
            {
                anyhow::bail!("waiting to reconnect to {}", self.address);
            }
            self.connect().await?;
        }

        let Some(connection) = self.connection.as_mut() else {
            anyhow::bail!("not connected");
        };

        match time::timeout(self.command_timeout, connection.cmd(command)).await {
            Ok(Ok(response)) => {
                self.link = Link::Up;
                Ok(response)
            }
            Ok(Err(error)) => {
                self.fail(format!("{command}: {error}"));
                Err(anyhow::anyhow!("`{command}` failed: {error}"))
            }
            Err(_) => {
                // A timed-out command leaves an unread response on the socket,
                // so the connection can no longer be trusted.
                self.fail(format!("`{command}` timed out"));
                Err(anyhow::anyhow!(
                    "`{command}` timed out after {:?}",
                    self.command_timeout
                ))
            }
        }
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.link = Link::Connecting;
        let target = self.address.to_string();

        let attempt = time::timeout(
            self.connect_timeout,
            Connection::<TcpStream>::connect(&target, &self.password),
        )
        .await;

        match attempt {
            Ok(Ok(connection)) => {
                self.connection = Some(connection);
                self.last_failure = None;
                self.link = Link::Up;
                Ok(())
            }
            Ok(Err(rcon::Error::Auth)) => {
                self.fail("authentication failed: check the RCON password".into());
                Err(anyhow::anyhow!(
                    "RCON authentication failed for {target}: check the password"
                ))
            }
            Ok(Err(error)) => {
                self.fail(format!("{error}"));
                Err(anyhow::anyhow!("connecting to {target}: {error}"))
            }
            Err(_) => {
                self.fail(format!(
                    "connection timed out after {:?}",
                    self.connect_timeout
                ));
                Err(anyhow::anyhow!("connecting to {target} timed out"))
            }
        }
    }

    fn fail(&mut self, reason: String) {
        self.connection = None;
        self.last_failure = Some(Instant::now());
        self.link = Link::Down(reason);
    }
}
