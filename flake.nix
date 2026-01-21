{
  description = "Multi-vendor network switch configurator";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Development dependencies
        nativeBuildInputs = with pkgs; [
          pkg-config
          openssl
        ];

        buildInputs = with pkgs; [
          openssl
          libiconv
        ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
          pkgs.darwin.apple_sdk.frameworks.Security
          pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
        ];

      in
      {
        # Development shell
        devShells.default = pkgs.mkShell {
          inherit buildInputs nativeBuildInputs;

          packages = with pkgs; [
            # Rust toolchain
            cargo
            rustc
            rust-analyzer
            rustfmt
            clippy

            # Cargo tools
            cargo-watch
            cargo-edit
            cargo-audit

            # Fast linkers for quicker builds
            mold        # Much faster linker than GNU ld
            clang       # Needed for mold

            # Additional tools
            jq
            yq
          ];

          shellHook = ''
            # Configure Rust to use mold linker for faster builds
            export RUSTFLAGS="-C link-arg=-fuse-ld=mold"

            echo "🦀 Rust development environment"
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo ""
            echo "⚡ Fast compilation enabled:"
            echo "  - Using mold linker (much faster than GNU ld)"
            echo "  - Parallel codegen enabled"
            echo ""
            echo "Available commands:"
            echo "  cargo build                    - Fast debug build"
            echo "  cargo build --profile dev-fast - Optimized debug build"
            echo "  cargo build --release          - Full optimization (slow)"
            echo "  cargo build --profile release-fast - Fast release build"
            echo "  cargo run                      - Run the application"
            echo "  cargo test                     - Run tests"
            echo "  cargo clippy                   - Run linter"
            echo "  cargo fmt                      - Format code"
            echo ""
            echo "To start Claude Code: claude-code ."
          '';
        };

        # Package - imported from package.nix
        packages.default = pkgs.callPackage ./package.nix {
          inherit nativeBuildInputs buildInputs;
        };

        # Alias for the package
        packages.switch-configurator = self.packages.${system}.default;

        # Apps
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/switch-configurator";
        };
      }
    ) // {
      # NixOS module - imported from nixos-module.nix
      nixosModules.default = import ./nixos-module.nix { inherit self; };

      # Alias for the module
      nixosModules.switch-configurator = self.nixosModules.default;

      # Nixpkgs overlay - allows adding this package to nixpkgs
      overlays.default = final: prev: {
        switch-configurator = final.callPackage ./package.nix {
          nativeBuildInputs = with final; [
            pkg-config
            openssl
          ];
          buildInputs = with final; [
            openssl
            libiconv
          ] ++ final.lib.optionals final.stdenv.isDarwin [
            final.darwin.apple_sdk.frameworks.Security
            final.darwin.apple_sdk.frameworks.SystemConfiguration
          ];
        };
      };

      # Alias for the overlay
      overlays.switch-configurator = self.overlays.default;
    };
}
