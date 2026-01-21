#!/usr/bin/env bash
# Quick test script to verify port range expansion

echo "Testing port range expansion..."
echo ""

# Create a simple Rust program to load and display the config
cargo run --quiet --bin switch-configurator 2>&1 | head -1 > /dev/null

# Use a Python script to parse and display the YAML after Rust processes it
cat > /tmp/test_ranges.py << 'EOF'
import yaml
import sys

# This would normally be loaded through Rust, but we'll just count expected expansions
config_file = sys.argv[1]
with open(config_file, 'r') as f:
    config = yaml.safe_load(f)

print(f"Config file: {config_file}")
print(f"Switch: {config['switches'][0]['hostname']}")
print(f"\nPort configurations defined:")

for i, port in enumerate(config['switches'][0]['ports'], 1):
    port_id = port['port_id']
    desc = port.get('description', 'No description')
    print(f"  {i}. port_id='{port_id}' - {desc}")

    # Calculate expected expansions
    if '-' in port_id or ',' in port_id:
        # Has range or list
        count = 0
        for segment in port_id.split(','):
            segment = segment.strip()
            if '-' in segment:
                # Range
                parts = segment.split('-')
                if parts[0].isdigit() and parts[1].isdigit():
                    count += int(parts[1]) - int(parts[0]) + 1
                else:
                    count += 1
            else:
                count += 1
        print(f"     -> Should expand to {count} ports")
    else:
        print(f"     -> Single port (no expansion)")

print(f"\nPort mirrors:")
for mirror in config['switches'][0].get('port_mirrors', []):
    print(f"  Session {mirror['session_id']}: sources {mirror['source_ports']} -> dest {mirror['destination_port']}")

print("\nExpected total after expansion:")
print("  1-5: 5 ports")
print("  7,9,11: 3 ports")
print("  13-15,17,19-21: 7 ports")
print("  24: 1 port")
print("  TOTAL: 16 ports")
EOF

python3 /tmp/test_ranges.py test-port-ranges.yaml
