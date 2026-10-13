{
  description = "Multi-vendor network switch configurator";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Development dependencies
        nativeBuildInputs = with pkgs; [
          pkg-config
          openssl
        ];

        buildInputs =
          with pkgs;
          [
            openssl
            libiconv
          ]
          ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
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
            mold # Much faster linker than GNU ld
            clang # Needed for mold

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

        # Aliases for the packages (both binaries are in the default package)
        packages.switch-configurator = self.packages.${system}.default;
        packages.switch-configurator-ui = self.packages.${system}.default;

        # Apps
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/switch-configurator";
        };

        apps.switch-configurator-ui = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/switch-configurator-ui";
        };

        # Evaluation-only module checks (no VM): cheap enough for `nix flake check`.
        checks = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          module-load-credentials =
            let
              eval = nixpkgs.lib.nixosSystem {
                inherit system;
                modules = [
                  self.nixosModules.default
                  {
                    services.switch-configurator = {
                      enable = true;
                      package = self.packages.${system}.default;
                      configFile = ./examples/config.example.yaml;
                      loadCredentials.snmp-ro = "/run/secrets/snmp-ro";
                    };
                    # Minimal host so the module system accepts the evaluation.
                    boot.loader.grub.enable = false;
                    fileSystems."/" = {
                      device = "none";
                      fsType = "tmpfs";
                    };
                    system.stateVersion = "24.11";
                  }
                ];
              };
              got = eval.config.systemd.services.switch-configurator.serviceConfig.LoadCredential;
              want = [ "snmp-ro:/run/secrets/snmp-ro" ];
            in
            pkgs.runCommand "module-load-credentials" { } (
              if got == want then
                "touch $out"
              else
                "echo 'LoadCredential: unexpected value ${builtins.toJSON got}' >&2; exit 1"
            );

          # A credential id that is a path would point outside $CREDENTIALS_DIRECTORY.
          module-load-credentials-rejects-path-ids =
            let
              eval = nixpkgs.lib.nixosSystem {
                inherit system;
                modules = [
                  self.nixosModules.default
                  {
                    services.switch-configurator = {
                      enable = true;
                      package = self.packages.${system}.default;
                      configFile = ./examples/config.example.yaml;
                      loadCredentials."../escape" = "/run/secrets/x";
                    };
                    boot.loader.grub.enable = false;
                    fileSystems."/" = {
                      device = "none";
                      fsType = "tmpfs";
                    };
                    system.stateVersion = "24.11";
                  }
                ];
              };
              rejected = builtins.any (a: !a.assertion) eval.config.assertions;
            in
            pkgs.runCommand "module-load-credentials-rejects-path-ids" { } (
              if rejected then "touch $out" else "echo 'path-like credential id was accepted' >&2; exit 1"
            );
        };
      }
    )
    // {
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
          buildInputs =
            with final;
            [
              openssl
              libiconv
            ]
            ++ final.lib.optionals final.stdenv.isDarwin [
              final.darwin.apple_sdk.frameworks.Security
              final.darwin.apple_sdk.frameworks.SystemConfiguration
            ];
        };
      };

      # Alias for the overlay
      overlays.switch-configurator = self.overlays.default;
    };
}
