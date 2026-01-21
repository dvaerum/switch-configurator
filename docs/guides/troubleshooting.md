# Troubleshooting

Common issues and their solutions.

## Connection Issues

### SSH Connection Fails

**Symptoms:**
```
ERROR Failed to connect: SSH connection error: Connection refused
```

**Solutions:**

1. **Verify switch is reachable:**
   ```bash
   ping 192.168.1.10
   ```

2. **Check SSH service is running:**
   ```bash
   ssh admin@192.168.1.10
   ```

3. **Verify credentials:**
   - Correct username/password
   - SSH key permissions: `chmod 600 /path/to/key`

4. **Check port:**
   ```yaml
   credentials:
     port: 22  # Default, verify switch SSH port
   ```

### Serial Connection Timeout

**Symptoms:**
```
ERROR Failed to connect: Timeout waiting for prompt
```

**Solutions:**

1. **Find serial device:**
   ```bash
   ls -l /dev/serial/by-id/
   ```

2. **Check device permissions:**
   ```bash
   ls -l /dev/ttyUSB0
   # Should show: crw-rw---- 1 root dialout
   ```

3. **Add user to dialout group:**
   ```bash
   sudo usermod -a -G dialout $USER
   # Logout and login for change to take effect
   ```

4. **Verify baud rate:**
   - Aruba: 9600
   - Cisco: 9600
   - Check switch documentation

5. **Test manually:**
   ```bash
   screen /dev/ttyUSB0 9600
   # Press Enter, you should see a prompt
   ```

### Permission Denied on Serial Device

**Symptoms:**
```
ERROR Permission denied: /dev/ttyUSB0
```

**Solutions:**

```bash
# Check current groups
groups

# Add to dialout group
sudo usermod -a -G dialout $USER

# Or change device permissions (temporary)
sudo chmod 666 /dev/ttyUSB0

# For NixOS, add udev rules (see NixOS deployment guide)
```

## Configuration Issues

### Invalid VLAN ID

**Symptoms:**
```
ERROR VLAN ID 5000 is invalid (must be 1-4094)
```

**Solution:**
VLAN IDs must be between 1 and 4094. Check your configuration.

### Port Not Found

**Symptoms:**
```
WARN Port '25' not found in current state
```

**Solutions:**

1. **Verify port exists on switch:**
   - Check switch model specifications
   - Aruba 2930F-24G has ports 1-24

2. **Check port ID format:**
   - Aruba: `"1"`, `"24"`
   - Cisco: `"GigabitEthernet1/0/1"`
   - FortiSwitch: `"port1"`

### YAML Parsing Error

**Symptoms:**
```
ERROR Failed to parse config: invalid type: integer `42`, expected a string
```

**Solutions:**

Ensure proper quoting:
```yaml
# Wrong
port_id: 1

# Correct
port_id: "1"
```

## Idempotency Issues

### Changes Keep Being Applied

**Symptoms:**
Running twice shows changes each time instead of "No changes needed".

**Debugging:**

```bash
# Run with debug logging
switch-configurator --config-file config.yaml --one-off --log-level debug

# Look for differences in parsed state
# Check lines with "Current:" vs "Desired:"
```

**Common causes:**

1. **Port mode mismatch:**
   - Parser detects Access, config says Trunk
   - Check allowed_vlans includes native VLAN

2. **VLAN IP config:**
   - Parser reads DHCP, config specifies none
   - Verify IP config in running config

3. **Description differences:**
   - Check for extra spaces or special characters

## Command Execution Issues

### Invalid Command

**Symptoms:**
```
WARN Command may have failed: configure terminal
Output contains error indication
```

**Solutions:**

1. **Enable debug logging:**
   ```bash
   switch-configurator --config-file config.yaml --one-off --log-level debug
   ```

2. **Test commands manually:**
   - SSH to switch
   - Try commands manually
   - Check for typos or syntax errors

3. **Check vendor documentation:**
   - Command syntax varies by vendor
   - Verify commands for your switch model

### Configuration Not Saved

**Symptoms:**
Changes apply but don't persist after reboot.

**Solutions:**

1. **Verify save command:**
   - Aruba: `write memory`
   - Cisco: `write memory` or `copy running-config startup-config`

2. **Check for errors:**
   ```bash
   switch-configurator --config-file config.yaml --one-off --log-level debug | grep -i save
   ```

## Performance Issues

### Slow Execution

**Causes:**

1. **Slow SSH connection:**
   - Network latency
   - DNS resolution delays

2. **Large configurations:**
   - Many VLANs or ports
   - Multiple switches

**Solutions:**

```bash
# Increase timeout
switch-configurator --config-file config.yaml --ssh-timeout 60

# Target specific switch
switch-configurator --config-file config.yaml --switch my-switch --one-off

# Use serial connection for initial setup
```

## Debugging Tips

### Enable Debug Logging

```bash
# Maximum verbosity
switch-configurator --config-file config.yaml --one-off --log-level debug

# Or set in config
settings:
  log_level: debug  # If supported in future versions
```

### Dry-Run Mode

Always test changes first:

```bash
switch-configurator --config-file config.yaml --one-off --dry-run
```

### Interactive Debug Mode

Step through commands:

```bash
switch-configurator --config-file config.yaml --one-off --debug
```

### Check Running Config

```bash
# Get current config from switch
curl http://localhost:4002/switches/my-switch/config

# Or manually
ssh admin@192.168.1.10 "show running-config"
```

### Verify State Parsing

```bash
# Look for parsing errors
switch-configurator --config-file config.yaml --one-off --log-level debug 2>&1 | grep -i "pars"
```

## Common Patterns

### Serial Connection Checklist

- [ ] Device exists: `ls /dev/ttyUSB0`
- [ ] Correct permissions: `ls -l /dev/ttyUSB0`
- [ ] User in dialout group: `groups`
- [ ] Correct baud rate in config
- [ ] Cable properly connected

### SSH Connection Checklist

- [ ] Switch pingable: `ping IP`
- [ ] SSH service running: `ssh user@IP`
- [ ] Correct credentials
- [ ] Firewall allows SSH
- [ ] Network routing correct

### Configuration Checklist

- [ ] YAML syntax valid
- [ ] Port IDs quoted as strings
- [ ] VLAN IDs in range 1-4094
- [ ] Switch model correct
- [ ] Credentials accurate

## Getting Help

If you're still stuck:

1. **Check logs:**
   ```bash
   # Service mode
   journalctl -u switch-configurator -n 100

   # One-off mode
   switch-configurator --config-file config.yaml --one-off --log-level debug 2>&1 | tee debug.log
   ```

2. **Verify configuration:**
   ```bash
   # Check YAML syntax
   yq . config.yaml

   # Validate against examples
   diff config.yaml examples/aruba-ssh.yaml
   ```

3. **Test connectivity:**
   ```bash
   # SSH
   ssh -v admin@192.168.1.10

   # Serial
   screen /dev/ttyUSB0 9600
   ```

4. **Check documentation:**
   - [Configuration Guide](configuration.md)
   - [Examples](../../examples/README.md)
   - [Architecture](../development/architecture.md)

5. **File an issue:**
   Include:
   - Switch model
   - Connection type
   - Configuration file (redact passwords)
   - Full error message
   - Debug logs

## See Also

- [Getting Started](getting-started.md) - Installation and setup
- [Configuration Guide](configuration.md) - Configuration reference
- [NixOS Deployment](../deployment/nixos.md) - NixOS-specific issues
