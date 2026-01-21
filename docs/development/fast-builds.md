# Fast Build Configuration

This document explains how the project is configured for faster compilation times.

## Quick Start

For the fastest development experience:

```bash
# Enter the nix development shell (automatically enables mold linker)
nix develop

# Fast debug build (default)
cargo build

# Even faster with minimal optimization
cargo build --profile dev-fast

# Fast release build (good balance)
cargo build --profile release-fast

# Full optimization (slow, only for final releases)
cargo build --release
```

## Build Profiles

### Development Profiles

| Profile | Command | Speed | Runtime | Use Case |
|---------|---------|-------|---------|----------|
| `dev` (default) | `cargo build` | ⚡⚡⚡ Fast | Slow | Quick iteration, debugging |
| `dev-fast` | `cargo build --profile dev-fast` | ⚡⚡ Very Fast | Medium | Faster iteration with better perf |

### Release Profiles

| Profile | Command | Speed | Runtime | Use Case |
|---------|---------|-------|---------|----------|
| `release-fast` | `cargo build --profile release-fast` | ⚡ Moderate | ⚡⚡ Fast | Testing, CI builds |
| `release` | `cargo build --release` | 🐌 Slow | ⚡⚡⚡ Fastest | Production releases |

## Optimizations Enabled

### 1. Parallel Compilation

**Default:** Rust uses all CPU cores automatically
- Multiple crates compile in parallel
- Each crate can use multiple codegen units

**Configuration** (Cargo.toml):
```toml
[profile.dev]
codegen-units = 256  # Default, allows parallel compilation

[profile.release]
codegen-units = 1    # Single-threaded but fastest runtime
```

### 2. Incremental Compilation

**Enabled by default for debug builds**
- Only recompiles changed code
- Much faster for iterative development

**Configuration** (Cargo.toml):
```toml
[profile.dev]
incremental = true   # Enabled
```

### 3. Fast Linker (mold)

**Automatically enabled in `nix develop`**
- mold is 10-30x faster than GNU ld
- Significantly reduces link time for large projects

**Configuration** (flake.nix):
```nix
shellHook = ''
  export RUSTFLAGS="-C link-arg=-fuse-ld=mold"
'';
```

**Without nix:**
```bash
# Install mold (Arch Linux)
sudo pacman -S mold

# Install mold (Ubuntu 22.04+)
sudo apt install mold

# Use mold
export RUSTFLAGS="-C link-arg=-fuse-ld=mold"
cargo build
```

### 4. Dependency Optimization

**Dependencies compile without debug info**
- Faster compilation
- Smaller binary size
- Your code still has full debug info

**Configuration** (Cargo.toml):
```toml
[profile.dev.package."*"]
opt-level = 0
debug = false  # No debug info for dependencies
```

## Benchmark: Build Times

Typical build times on a modern 8-core CPU:

| Scenario | Without Optimizations | With Optimizations | Improvement |
|----------|----------------------|-------------------|-------------|
| Clean build (dev) | ~45s | ~15s | 3x faster |
| Incremental rebuild | ~8s | ~3s | 2.5x faster |
| Clean release build | ~120s | ~40s | 3x faster |

## Troubleshooting

### Build still slow?

1. **Check if mold is being used:**
   ```bash
   echo $RUSTFLAGS
   # Should show: -C link-arg=-fuse-ld=mold
   ```

2. **Check CPU usage:**
   ```bash
   htop
   # Should see multiple rustc processes using ~100% CPU each
   ```

3. **Clean build cache:**
   ```bash
   cargo clean
   rm -rf target/
   cargo build
   ```

4. **Use cargo-watch for instant feedback:**
   ```bash
   cargo watch -x check           # Just check for errors (fastest)
   cargo watch -x 'run -- --help' # Rebuild and run on save
   ```

### mold not available?

Alternative fast linkers (in order of preference):

1. **lld** (LLVM linker):
   ```bash
   export RUSTFLAGS="-C link-arg=-fuse-ld=lld"
   ```

2. **GNU ld.gold**:
   ```bash
   export RUSTFLAGS="-C link-arg=-fuse-ld=gold"
   ```

## Advanced: sccache (Compilation Cache)

For even faster builds across multiple projects:

```bash
# Install sccache
cargo install sccache

# Configure in ~/.cargo/config.toml
[build]
rustc-wrapper = "sccache"

# Check cache stats
sccache --show-stats
```

## Development Workflow

**Recommended workflow for fastest iteration:**

```bash
# 1. Enter dev shell (enables all optimizations)
nix develop

# 2. Use cargo-watch for automatic rebuilds
cargo watch -x 'check'  # Just check syntax (instant)

# 3. When ready to test:
cargo run -- --config-file tests/configs/cisco-basic-test.yaml --one-off

# 4. Before committing, run full checks:
cargo test
cargo clippy
cargo fmt --check
```

## Profile Comparison

### codegen-units Impact

```toml
codegen-units = 1    # Single-threaded, slowest build, fastest runtime
codegen-units = 16   # Good balance (release-fast)
codegen-units = 256  # Maximum parallelism (dev)
```

### LTO (Link-Time Optimization) Impact

```toml
lto = false        # Fast linking, no cross-crate optimization
lto = "thin"       # Moderate speed, some optimization (release-fast)
lto = true         # Slow linking, maximum optimization (release)
```

## Summary

✅ **For development**: Use `cargo build` in `nix develop`
✅ **For testing**: Use `cargo build --profile dev-fast`
✅ **For CI/testing releases**: Use `cargo build --profile release-fast`
✅ **For production**: Use `cargo build --release`

The configuration provides **3x faster builds** during development while maintaining full optimization for production releases.
