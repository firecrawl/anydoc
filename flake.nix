{
  description = "anydoc — convert documents (docx, pptx, xlsx, odt, rtf, epub, pdf, csv) to GitHub-Flavored Markdown, fully offline";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      version = (nixpkgs.lib.importTOML ./Cargo.toml).package.version;
    in {
      packages = forAllSystems (pkgs: rec {
        # Bare Rust CLI: the `convert` example over the local library, no runtime
        # deps. This is the direct path — a single static-ish binary for scape VMs
        # and ops workflows that just need file-in / markdown-out.
        anydoc = pkgs.rustPlatform.buildRustPackage {
          pname = "anydoc";
          inherit version;

          src = ./.;

          cargoLock.lockFile = ./Cargo.lock;

          # The crate ships as a library plus napi/pyo3/wasm bindings; the runnable
          # CLI is the `convert` example, a thin arg-parser over the public
          # `to_markdown_bytes`/`to_document` functions. Build just the root crate
          # and that example — never the binding members, which need node/python/wasm
          # toolchains that aren't present here.
          cargoBuildFlags = [ "--package" "anydoc" "--example" "convert" ];

          # Snapshot tests depend on a gitignored `samples/` corpus that isn't in
          # the source tree, so the default `cargo test` can't run here.
          doCheck = false;

          # buildRustPackage only installs top-level `target/*/release` binaries;
          # the example lands under `examples/`, so place it by hand as `anydoc`.
          postInstall = ''
            install -Dm755 \
              "$(find target -type f -name convert -path '*/release/examples/*' | head -n1)" \
              "$out/bin/anydoc"
          '';

          meta = with pkgs.lib; {
            description = "Convert documents to GitHub-Flavored Markdown, fully offline";
            homepage = "https://github.com/getmissionctrl/anydoc";
            license = licenses.mit;
            mainProgram = "anydoc";
            platforms = systems;
          };
        };

        # Node CLI: the same local Rust conversion behind the napi binding, driven
        # by the upstream `cli.js` for the nicer experience (--help, --version,
        # stdin `-`, richer format detection). Pulls in a node runtime, so it's the
        # opt-in package for consumers who want the polish over the bare exe.
        anydoc-node = pkgs.rustPlatform.buildRustPackage {
          pname = "anydoc-node";
          inherit version;

          src = ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];

          # Build only the napi cdylib member — not the root bin or the other
          # bindings. The resulting shared object is loaded by index.js.
          cargoBuildFlags = [ "--package" "anydoc-node" ];

          doCheck = false;

          # Assemble the JS package next to the freshly built native addon and
          # point the loader straight at it via NAPI_RS_NATIVE_LIBRARY_PATH
          # (index.js honours that env var first), sidestepping the platform-triple
          # filename dance. The wrapper is the `anydoc` command.
          postInstall = ''
            libdir="$out/libexec/anydoc-node"
            mkdir -p "$libdir"
            cp node/index.js node/index.d.ts node/cli.js node/package.json "$libdir/"
            cp "$(find target -type f \( -name 'libanydoc_node.so' -o -name 'libanydoc_node.dylib' \) -path '*/release/*' | head -n1)" \
              "$libdir/anydoc.node"

            makeWrapper ${pkgs.lib.getExe pkgs.nodejs} "$out/bin/anydoc" \
              --add-flags "$libdir/cli.js" \
              --set NAPI_RS_NATIVE_LIBRARY_PATH "$libdir/anydoc.node"
          '';

          meta = with pkgs.lib; {
            description = "anydoc Node CLI — document-to-Markdown with --help/--version/stdin, fully offline";
            homepage = "https://github.com/getmissionctrl/anydoc";
            license = licenses.mit;
            mainProgram = "anydoc";
            platforms = systems;
          };
        };

        default = anydoc;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixpkgs-fmt);
    };
}
