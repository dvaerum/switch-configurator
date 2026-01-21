{ pkgs
, nativeBuildInputs
, buildInputs
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
pkgs.rustPlatform.buildRustPackage {
  pname = "switch-configurator";
  version = cargoToml.package.version;

  # Only include files needed for the Rust build to avoid unnecessary rebuilds
  # Excludes: nixos-module.nix, flake.nix, docs/, examples/, *.md, etc.
  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    filter = path: type:
      let
        baseName = baseNameOf path;
        relPath = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
      in
        # Include Cargo files
        baseName == "Cargo.toml" ||
        baseName == "Cargo.lock" ||
        baseName == "build.rs" ||
        # Include src directory and all its contents
        (pkgs.lib.hasPrefix "src/" relPath || baseName == "src") ||
        # Include any .rs files at root level
        (type == "regular" && pkgs.lib.hasSuffix ".rs" baseName);
  };

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  inherit nativeBuildInputs buildInputs;

  # Skip tests during build (they may require network/SSH)
  doCheck = false;

  meta = with pkgs.lib; {
    description = "Multi-vendor network switch configurator";
    homepage = "https://github.com/yourusername/switch-configurator";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "switch-configurator";
  };
}
