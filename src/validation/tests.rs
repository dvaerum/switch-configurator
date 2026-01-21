use super::{TestFailure, TestType, ValidationTest};
use crate::ssh::connection::ConnectionClient;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Execute a validation test
pub async fn execute_test(
    test: &ValidationTest,
    client: &mut ConnectionClient,
    management_ip: &str,
) -> Result<(), TestFailure> {
    let start = Instant::now();

    let result = match &test.test_type {
        TestType::CommandExecution { command } => {
            execute_command_test(client, command).await
        }
        TestType::PingManagementIp { count } => {
            execute_ping_management_test(management_ip, *count).await
        }
        TestType::TcpPortCheck { ports } => {
            execute_tcp_port_test(management_ip, ports).await
        }
        TestType::GatewayReachable { target, source_vlan } => {
            execute_gateway_test(client, target, *source_vlan).await
        }
        TestType::DeviceReachable { device_ip, device_name, count } => {
            execute_device_reachable_test(client, device_ip, device_name, *count).await
        }
        TestType::PortStatus { critical_ports } => {
            execute_port_status_test(client, critical_ports).await
        }
        TestType::VlanMembership => {
            execute_vlan_membership_test(client).await
        }
    };

    match result {
        Ok(()) => {
            let duration = start.elapsed();
            debug!("Test passed: {:?} (took {:?})", test.test_type, duration);
            Ok(())
        }
        Err(error) => {
            let duration = start.elapsed();
            warn!("Test failed: {:?} - {} (took {:?})", test.test_type, error, duration);
            Err(TestFailure {
                test_name: format!("{:?}", test.test_type),
                required: test.required,
                error,
                duration,
            })
        }
    }
}

/// Test 1: Command Execution
/// Verify we can still execute commands on the switch (connection is working)
async fn execute_command_test(
    client: &mut ConnectionClient,
    command: &str,
) -> Result<(), String> {
    info!("Running command execution test: {}", command);

    let output = client
        .execute_command(command)
        .await
        .map_err(|e| format!("Failed to execute command '{}': {}", command, e))?;

    // Check if output contains error indicators
    let output_lower = output.to_lowercase();
    if output_lower.contains("error")
        || output_lower.contains("invalid")
        || output_lower.contains("unknown command") {
        return Err(format!("Command '{}' returned error: {}", command, output));
    }

    // Check if output is empty (might indicate communication issue)
    if output.trim().is_empty() {
        return Err(format!("Command '{}' returned empty output", command));
    }

    info!("✓ Command execution test passed");
    Ok(())
}

/// Test 2: Ping Management IP
/// Ping the switch from the host running switch-configurator
async fn execute_ping_management_test(
    management_ip: &str,
    count: u32,
) -> Result<(), String> {
    info!("Running ping test to management IP: {} ({} pings)", management_ip, count);

    // Use the ping command from the host system
    let output = Command::new("ping")
        .arg("-c")
        .arg(count.to_string())
        .arg("-W")
        .arg("2") // 2 second timeout per ping
        .arg(management_ip)
        .output()
        .map_err(|e| format!("Failed to execute ping command: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Ping to {} failed: {}", management_ip, stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for packet loss
    if stdout.contains("100% packet loss") || stdout.contains("100.0% packet loss") {
        return Err(format!("Ping to {} failed: 100% packet loss", management_ip));
    }

    info!("✓ Ping test passed");
    Ok(())
}

/// Test 3: TCP Port Check
/// Verify specific TCP ports are accessible on the switch
async fn execute_tcp_port_test(
    management_ip: &str,
    ports: &[u16],
) -> Result<(), String> {
    info!("Running TCP port check for {} ports", ports.len());

    let mut failed_ports = Vec::new();

    for &port in ports {
        let addr = format!("{}:{}", management_ip, port);

        debug!("Testing TCP connection to {}", addr);

        // Try to establish TCP connection with 5 second timeout
        let result = TcpStream::connect_timeout(
            &addr.to_socket_addrs()
                .map_err(|e| format!("Failed to resolve {}: {}", addr, e))?
                .next()
                .ok_or_else(|| format!("No address found for {}", addr))?,
            Duration::from_secs(5),
        );

        match result {
            Ok(_) => {
                debug!("✓ Port {} is accessible", port);
            }
            Err(e) => {
                warn!("✗ Port {} is not accessible: {}", port, e);
                failed_ports.push(port);
            }
        }
    }

    if !failed_ports.is_empty() {
        return Err(format!(
            "TCP port check failed for ports: {:?}",
            failed_ports
        ));
    }

    info!("✓ TCP port check passed");
    Ok(())
}

/// Test 4: Gateway Reachable
/// Ping a gateway/target from the switch itself
async fn execute_gateway_test(
    client: &mut ConnectionClient,
    target: &str,
    source_vlan: Option<u16>,
) -> Result<(), String> {
    info!("Running gateway reachability test to {}", target);

    // Build ping command based on source VLAN
    let ping_command = if let Some(vlan) = source_vlan {
        // Aruba syntax: ping vlan <id> <target>
        // This might need to be vendor-specific in the future
        format!("ping {} source vlan {}", target, vlan)
    } else {
        format!("ping {} count 3", target)
    };

    debug!("Executing: {}", ping_command);

    let output = client
        .execute_command(&ping_command)
        .await
        .map_err(|e| format!("Failed to execute ping command: {}", e))?;

    // Check for success indicators in output
    let output_lower = output.to_lowercase();
    if output_lower.contains("100% packet loss")
        || output_lower.contains("unreachable")
        || output_lower.contains("timed out")
        || output_lower.contains("failed") {
        return Err(format!("Ping to {} failed: {}", target, output.lines().next().unwrap_or("unknown error")));
    }

    // Look for success indicators
    if !output_lower.contains("reply") && !output_lower.contains("received") && !output_lower.contains("alive") {
        return Err(format!("Ping to {} returned unexpected output: {}", target, output));
    }

    info!("✓ Gateway reachability test passed");
    Ok(())
}

/// Test 5: Device Reachable
/// Test if a specific device/IP is reachable from the switch
async fn execute_device_reachable_test(
    client: &mut ConnectionClient,
    device_ip: &str,
    device_name: &str,
    count: u32,
) -> Result<(), String> {
    info!("Running device reachability test for {} ({})", device_name, device_ip);

    let ping_command = format!("ping {} count {}", device_ip, count);

    debug!("Executing: {}", ping_command);

    let output = client
        .execute_command(&ping_command)
        .await
        .map_err(|e| format!("Failed to ping {} ({}): {}", device_name, device_ip, e))?;

    // Check for failures
    let output_lower = output.to_lowercase();
    if output_lower.contains("100% packet loss")
        || output_lower.contains("unreachable")
        || output_lower.contains("timed out") {
        return Err(format!("Device {} ({}) is not reachable", device_name, device_ip));
    }

    // Look for success indicators
    if !output_lower.contains("reply") && !output_lower.contains("received") && !output_lower.contains("alive") {
        return Err(format!("Device {} ({}) returned unexpected ping output", device_name, device_ip));
    }

    info!("✓ Device reachability test passed for {}", device_name);
    Ok(())
}

/// Test 6: Port Status
/// Verify critical ports are in the expected state (up/down)
async fn execute_port_status_test(
    client: &mut ConnectionClient,
    critical_ports: &[String],
) -> Result<(), String> {
    if critical_ports.is_empty() {
        debug!("No critical ports specified, skipping port status test");
        return Ok(());
    }

    info!("Running port status test for {} critical ports", critical_ports.len());

    // Execute show interfaces command (vendor-specific, but similar across vendors)
    let output = match client.execute_command("show interfaces brief").await {
        Ok(output) => output,
        Err(_) => {
            // Try alternative command
            client
                .execute_command("show interface status")
                .await
                .map_err(|e| format!("Failed to get interface status: {}", e))?
        }
    };

    let output_lower = output.to_lowercase();
    let mut failed_ports = Vec::new();

    for port in critical_ports {
        let port_lower = port.to_lowercase();

        // Look for the port in the output
        let port_found = output_lower.contains(&port_lower);

        if !port_found {
            failed_ports.push(format!("{} (not found in output)", port));
            continue;
        }

        // Check if port is down/disabled
        // Look for lines containing the port and check status
        for line in output.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.contains(&port_lower) {
                if line_lower.contains("down") || line_lower.contains("disabled") {
                    failed_ports.push(format!("{} (down)", port));
                }
                break;
            }
        }
    }

    if !failed_ports.is_empty() {
        return Err(format!("Critical ports are not up: {:?}", failed_ports));
    }

    info!("✓ Port status test passed");
    Ok(())
}

/// Test 7: VLAN Membership
/// Verify VLAN configuration is correct (basic check)
async fn execute_vlan_membership_test(
    client: &mut ConnectionClient,
) -> Result<(), String> {
    info!("Running VLAN membership test");

    // Execute show vlan command
    let output = match client.execute_command("show vlan").await {
        Ok(output) => output,
        Err(_) => {
            // Try alternative command
            client
                .execute_command("show vlans")
                .await
                .map_err(|e| format!("Failed to get VLAN information: {}", e))?
        }
    };

    // Basic check: make sure we got VLAN output
    let output_lower = output.to_lowercase();
    if output_lower.contains("error") || output_lower.contains("invalid") {
        return Err(format!("VLAN command returned error: {}", output));
    }

    // Check that output contains "vlan" to verify command worked
    if !output_lower.contains("vlan") {
        return Err("VLAN command returned unexpected output".to_string());
    }

    info!("✓ VLAN membership test passed");
    Ok(())
}
