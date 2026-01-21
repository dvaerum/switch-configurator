# Overlay Usage Examples
# This file demonstrates how to use the switch-configurator overlay

# Method 1: Using the overlay in a flake-based NixOS configuration
# In your flake.nix:
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    switch-configurator.url = "github:yourusername/switch-configurator";
  };

  outputs = { self, nixpkgs, switch-configurator }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        # Apply overlay
        ({ config, pkgs, ... }: {
          nixpkgs.overlays = [
            switch-configurator.overlays.default
          ];
        })

        # Now you can use the package
        ({ config, pkgs, ... }: {
          environment.systemPackages = [ pkgs.switch-configurator ];

          # Or use the NixOS module
          imports = [ switch-configurator.nixosModules.default ];
          services.switch-configurator = {
            enable = true;
            configFile = /etc/switch-configurator/config.yaml;
          };
        })
      ];
    };
  };
}

# Method 2: Using the overlay without the NixOS module
# In your configuration.nix or flake:
{
  nixpkgs.overlays = [
    (import (builtins.fetchTarball "https://github.com/yourusername/switch-configurator/archive/main.tar.gz")).overlays.default
  ];

  environment.systemPackages = with pkgs; [
    switch-configurator
  ];
}

# Method 3: Standalone overlay for non-NixOS systems
# In ~/.config/nixpkgs/overlays.nix or ~/.config/nixpkgs/overlays/switch-configurator.nix:
let
  switch-configurator-overlay = (import (builtins.fetchTarball {
    url = "https://github.com/yourusername/switch-configurator/archive/main.tar.gz";
  })).overlays.default;
in
[
  switch-configurator-overlay
]

# Then install with:
# nix-env -iA nixpkgs.switch-configurator

# Method 4: Using in a shell.nix for development
# shell.nix:
let
  switch-configurator-src = builtins.fetchGit {
    url = "https://github.com/yourusername/switch-configurator";
    ref = "main";
  };
  switch-configurator-overlay = (import switch-configurator-src).overlays.default;

  pkgs = import <nixpkgs> {
    overlays = [ switch-configurator-overlay ];
  };
in
pkgs.mkShell {
  buildInputs = with pkgs; [
    switch-configurator
    jq
    yq
  ];
}

# Method 5: Using the package directly in a derivation
# your-package.nix:
{ pkgs ? import <nixpkgs> { }
}:

let
  switch-configurator-flake = builtins.getFlake "github:yourusername/switch-configurator";
  switch-configurator = switch-configurator-flake.packages.${pkgs.system}.default;
in
pkgs.stdenv.mkDerivation {
  name = "my-network-config";
  buildInputs = [ switch-configurator ];
  # ... rest of your derivation
}

# Method 6: Override or extend the package
{
  nixpkgs.overlays = [
    switch-configurator.overlays.default

    # Custom overlay to override switch-configurator
    (final: prev: {
      switch-configurator = prev.switch-configurator.overrideAttrs (oldAttrs: {
        # Add custom patches
        patches = (oldAttrs.patches or []) ++ [ ./my-custom.patch ];

        # Or change build flags
        buildInputs = oldAttrs.buildInputs ++ [ final.someOtherPackage ];
      });
    })
  ];
}

# Verifying the overlay works:
# nix-instantiate --eval -E '(import <nixpkgs> { overlays = [ /* your overlays */ ]; }).switch-configurator.name'
# Should output: "switch-configurator-0.3.0"

# Building from the overlay:
# nix-build '<nixpkgs>' -A switch-configurator --option extra-experimental-features 'flakes nix-command'
