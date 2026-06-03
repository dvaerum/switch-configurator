#!/usr/bin/env bash
# Playwright MCP server using system chromium from NixOS
# The --executable-path flag bypasses playwright's browser manager entirely
CHROMIUM_PATH=$(nix-shell -p chromium --run "which chromium" 2>/dev/null)
export PLAYWRIGHT_BROWSERS_PATH=/tmp/pw-browsers-writable
mkdir -p "$PLAYWRIGHT_BROWSERS_PATH"
exec nix-shell -p nodejs chromium --run "npx @playwright/mcp@0.0.40 --headless --executable-path $CHROMIUM_PATH"
