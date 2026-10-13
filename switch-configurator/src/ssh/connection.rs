use anyhow::Result;
use super::{SshClient, SerialClient};

/// Unified client that supports both SSH and Serial connections
pub enum ConnectionClient {
    Ssh(SshClient),
    Serial(SerialClient),
}

impl ConnectionClient {
    /// Execute a single command
    pub async fn execute_command(&mut self, command: &str) -> Result<String> {
        match self {
            Self::Ssh(client) => client.execute_command(command).await,
            Self::Serial(client) => client.execute_command(command).await,
        }
    }

    /// Execute multiple commands
    pub async fn execute_commands(&mut self, commands: &[String]) -> Result<Vec<String>> {
        match self {
            Self::Ssh(client) => client.execute_commands(commands).await,
            Self::Serial(client) => client.execute_commands(commands).await,
        }
    }

    /// Is this kept-open session still logged in and responsive? Serial gets
    /// a prompt-only probe; SSH has no cheap equivalent, so a no-op command
    /// that fails if the channel is gone.
    pub async fn probe(&mut self) -> Result<()> {
        match self {
            Self::Ssh(client) => client.execute_command("end").await.map(|_| ()),
            Self::Serial(client) => client.probe_session().await,
        }
    }

    /// Disconnect from the device
    pub async fn disconnect(&mut self) -> Result<()> {
        match self {
            Self::Ssh(client) => client.disconnect().await,
            Self::Serial(client) => client.disconnect().await,
        }
    }
}
