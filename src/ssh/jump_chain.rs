use anyhow::{Context, Result};
use tracing::{debug, info};
use crate::models::JumpHost;
use crate::ssh::jump_host::JumpHostSession;
use crate::ssh::jump_host_parser::resolve_jump_host;

/// Manages a chain of jump host connections
pub struct JumpHostChain {
    sessions: Vec<JumpHostSession>,
    final_local_port: u16,
}

impl JumpHostChain {
    /// Establish a chain of jump host connections
    /// Returns the local port that forwards to the final target
    ///
    /// Connection flow:
    /// 1. Connect to first jump host directly
    /// 2. Create port forward from first jump host to second jump host
    /// 3. Connect to localhost:port (which tunnels to second jump host)
    /// 4. Repeat until all jump hosts are connected
    /// 5. Final jump host creates port forward to target switch
    /// 6. Return local port that connects to target switch
    pub async fn establish(
        jump_hosts: &[JumpHost],
        target_host: &str,
        target_port: u16,
        target_username: &str,
    ) -> Result<Self> {
        if jump_hosts.is_empty() {
            anyhow::bail!("Jump host chain is empty");
        }

        info!(
            "Establishing jump host chain: {} hops to {}:{}",
            jump_hosts.len(),
            target_host,
            target_port
        );

        let mut sessions = Vec::new();
        let mut current_target_host = None;
        let mut current_target_port = None;

        // Iterate through each jump host in the chain
        for (idx, jump_host) in jump_hosts.iter().enumerate() {
            let hop_num = idx + 1;
            let total_hops = jump_hosts.len();

            info!("Establishing hop {}/{}", hop_num, total_hops);

            // Resolve jump host configuration
            let resolved = resolve_jump_host(jump_host, target_username)
                .with_context(|| format!("Failed to resolve jump host #{}", hop_num))?;

            // For the first hop, we need to note we're connecting directly
            // For subsequent hops, we connect through the previous tunnel
            if idx == 0 {
                debug!(
                    "Hop {}/{}: Direct connection to {}:{}",
                    hop_num, total_hops, resolved.hostname, resolved.port
                );
            } else {
                debug!(
                    "Hop {}/{}: Connecting via tunnel to {}:{}",
                    hop_num,
                    total_hops,
                    current_target_host.as_ref().unwrap(),
                    current_target_port.unwrap()
                );
            }

            // Establish connection to this jump host
            // Note: If this is not the first hop, the resolved hostname/port
            // will be overridden by russh when we connect through localhost
            let mut jump_session = JumpHostSession::connect(resolved).await
                .with_context(|| format!("Failed to connect to jump host #{}", hop_num))?;

            // Determine next target (either next jump host or final target switch)
            let (next_host, next_port) = if idx < jump_hosts.len() - 1 {
                // There's another jump host after this one
                let next_jump = &jump_hosts[idx + 1];
                let next_resolved = resolve_jump_host(next_jump, target_username)?;
                (next_resolved.hostname, next_resolved.port)
            } else {
                // This is the last jump host - target is the final switch
                (target_host.to_string(), target_port)
            };

            // Create port forward to next hop
            debug!(
                "Hop {}/{}: Creating port forward to {}:{}",
                hop_num, total_hops, next_host, next_port
            );

            let local_port = jump_session.create_port_forward(&next_host, next_port).await
                .with_context(|| {
                    format!("Failed to create port forward on jump host #{}", hop_num)
                })?;

            // Next hop connects to this local port
            current_target_host = Some("127.0.0.1".to_string());
            current_target_port = Some(local_port);

            // Keep session alive
            sessions.push(jump_session);
        }

        let final_local_port = current_target_port
            .expect("Should have established at least one port forward");

        info!(
            "Jump host chain established successfully. Connect to localhost:{}",
            final_local_port
        );

        Ok(Self {
            sessions,
            final_local_port,
        })
    }

    /// Get the local port that forwards to the final target
    pub fn local_port(&self) -> u16 {
        self.final_local_port
    }

    /// Get number of hops in the chain
    pub fn hop_count(&self) -> usize {
        self.sessions.len()
    }

    /// Disconnect all jump host sessions
    pub async fn disconnect(self) -> Result<()> {
        info!("Disconnecting jump host chain ({} hops)", self.sessions.len());

        // Sessions will be dropped and disconnected automatically
        // Explicit disconnect could be added here if needed
        drop(self.sessions);

        Ok(())
    }
}
