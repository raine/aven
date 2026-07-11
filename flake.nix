{
  description = "Local-first task manager CLI and sync server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.stdenv.mkDerivation {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;

            src = ./.;

            nativeBuildInputs = with pkgs; [
              cargo
              rustc
              pkg-config
            ];
            buildInputs = [ pkgs.sqlite ];

            buildPhase = ''
              runHook preBuild
              export CARGO_HOME="$TMPDIR/cargo-home"
              cargo build --release --locked --ignore-rust-version
              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              install -Dm755 target/release/aven "$out/bin/aven"
              runHook postInstall
            '';

            meta = with pkgs.lib; {
              description = "Local-first task manager CLI and sync server";
              homepage = "https://github.com/raine/aven";
              license = licenses.mit;
              mainProgram = "aven";
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/aven";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              rustfmt
              clippy
              pkg-config
              sqlite
            ];

            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          };
        }
      );
    };
}
