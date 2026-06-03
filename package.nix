{ pkgs
, nativeBuildInputs
, buildInputs
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile ./switch-configurator/Cargo.toml);
in
pkgs.rustPlatform.buildRustPackage {
  pname = "switch-configurator";
  version = cargoToml.package.version;

  src = pkgs.lib.cleanSourceWith {
    src = ./.;
    filter = path: type:
      let
        baseName = baseNameOf path;
        relPath = pkgs.lib.removePrefix (toString ./. + "/") (toString path);
      in
        # Include workspace root Cargo files
        baseName == "Cargo.toml" ||
        baseName == "Cargo.lock" ||
        # Include backend crate directory and all contents
        baseName == "switch-configurator" ||
        (pkgs.lib.hasPrefix "switch-configurator/" relPath) ||
        # Include UI crate directory and all contents
        baseName == "switch-configurator-ui" ||
        (pkgs.lib.hasPrefix "switch-configurator-ui/" relPath);
  };

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  inherit nativeBuildInputs buildInputs;

  doCheck = false;

  meta = with pkgs.lib; {
    description = "Multi-vendor network switch configurator with web UI";
    homepage = "https://github.com/yourusername/switch-configurator";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "switch-configurator";
  };
}
