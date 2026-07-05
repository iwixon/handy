{
  description = "Handy - A free, open source, and extensible speech-to-text application that works completely offline";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # bun2nix: generates per-package Nix fetchurl expressions from bun.lock,
    # replacing the old FOD approach where a single hash covered the entire
    # node_modules directory (that hash would break on bun version changes).
    # See: https://github.com/nix-community/bun2nix
    bun2nix = {
      url = "github:nix-community/bun2nix/2.0.8";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      bun2nix,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      # Read version from Cargo.toml
      cargoToml = fromTOML (builtins.readFile ./src-tauri/Cargo.toml);
      version = cargoToml.package.version;

      # Shared native library dependencies for both package build and dev shell.
      # Keep in sync: if a native dep is needed for compilation, add it here.
      commonNativeDeps = pkgs:
        let
          inherit (pkgs.stdenv) isDarwin isLinux;
        in
        pkgs.lib.optionals isLinux (with pkgs; [
          webkitgtk_4_1
          gtk3
          glib
          libsoup_3
          alsa-lib
          libayatana-appindicator
          libevdev
          libxtst
          gtk-layer-shell
          vulkan-loader
          vulkan-headers
          shaderc
        ])
        ++ pkgs.lib.optionals isDarwin [
          pkgs.apple-sdk_15
          pkgs.darwin.cctools
        ]
        ++ (with pkgs; [
          onnxruntime
          openssl
        ]);

      # GStreamer plugins for WebKitGTK audio/video (Linux only)
      gstPlugins = pkgs: with pkgs.gst_all_1; [
        gstreamer
        gst-plugins-base
        gst-plugins-good
        gst-plugins-bad
        gst-plugins-ugly
      ];

      # Shared environment variables for Rust/native builds.
      # LIBCLANG_PATH / BINDGEN_EXTRA_CLANG_ARGS are handled by
      # rustPlatform.bindgenHook (see nativeBuildInputs), not set here.
      commonEnv = pkgs:
        let
          lib = pkgs.lib;
          inherit (pkgs.stdenv) isLinux;
        in
        {
          ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";
          ORT_PREFER_DYNAMIC_LINK = "1";
        }
        // lib.optionalAttrs isLinux {
          GST_PLUGIN_SYSTEM_PATH_1_0 = "${lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" (gstPlugins pkgs)}";
        };

    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [
              bun2nix.overlays.default
            ];
          };
          lib = pkgs.lib;
          inherit (pkgs.stdenv) isDarwin isLinux;
          combinedAlsaPlugins = pkgs.symlinkJoin {
            name = "combined-alsa-plugins";
            paths = [
              "${pkgs.pipewire}/lib/alsa-lib"
              "${pkgs.alsa-plugins}/lib/alsa-lib"
            ];
          };
        in
        {
          handy = pkgs.rustPlatform.buildRustPackage (
            {
              pname = "handy";
              inherit version;
              src = self;

              cargoRoot = "src-tauri";
              buildAndTestSubdir = "src-tauri";
              # cargo-tauri.hook builds this bundle type and its standard
              # install phase places the artifacts. macOS has no .deb, so it
              # builds an .app bundle and uses the custom installPhase below.
              tauriBundleType = if isDarwin then "app" else "deb";

              cargoLock = {
                lockFile = ./src-tauri/Cargo.lock;
                # Automatically fetch git dependencies using builtins.fetchGit.
                # This eliminates the need for manual outputHashes that had to be
                # updated every time a git dependency changed in Cargo.lock.
                # Safe for standalone flakes (not allowed in nixpkgs, it is needed something like crate2nix).
                allowBuiltinFetchGit = true;
              };

              postPatch = ''
                ${pkgs.jq}/bin/jq '.bundle.createUpdaterArtifacts = false' \
                  src-tauri/tauri.conf.json > $TMPDIR/tauri.conf.json
                cp $TMPDIR/tauri.conf.json src-tauri/tauri.conf.json

                # Strip postinstall hook — it runs check-nix-deps.ts which is only
                # needed during local development, not inside the Nix sandbox.
                ${pkgs.jq}/bin/jq 'del(.scripts.postinstall)' \
                  package.json > $TMPDIR/package.json
                cp $TMPDIR/package.json package.json
              ''
              + lib.optionalString isLinux ''
                # Point libappindicator-sys to the Nix store path
                substituteInPlace \
                  $cargoDepsCopy/libappindicator-sys-*/src/lib.rs \
                  --replace-fail \
                    "libayatana-appindicator3.so.1" \
                    "${pkgs.libayatana-appindicator}/lib/libayatana-appindicator3.so.1"
              ''
              + lib.optionalString isDarwin (
                # Replace Swift Apple Intelligence bridge with a C stub for the
                # Nix sandbox (xcrun/swiftc aren't available). The stub returns
                # "not available" so the feature is disabled at runtime, but all
                # FFI symbols resolve at link time.
                let
                  cStub = pkgs.writeText "apple_intelligence_nix_stub.c" ''
                    #include <stdlib.h>
                    typedef struct { int success; char* text; char* error; } AppleLLMResponse;
                    int is_apple_intelligence_available(void) { return 0; }
                    AppleLLMResponse* process_text_with_system_prompt_apple(
                        const char* sp, const char* uc, int mt) { return NULL; }
                    void free_apple_llm_response(AppleLLMResponse* r) {}
                  '';
                  rsStub = pkgs.writeText "apple_intelligence_c_stub.rs" ''
                    fn build_apple_intelligence_c_stub() {
                        let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
                        let status = std::process::Command::new("cc")
                            .args(["-c", "${cStub}", "-o"])
                            .arg(out_dir.join("apple_intelligence.o"))
                            .status()
                            .expect("Failed to compile Apple Intelligence C stub");
                        assert!(status.success(), "cc failed to compile stub");
                        let status = std::process::Command::new("ar")
                            .args(["rcs"])
                            .arg(out_dir.join("libapple_intelligence.a"))
                            .arg(out_dir.join("apple_intelligence.o"))
                            .status()
                            .expect("Failed to create static library");
                        assert!(status.success(), "ar failed");
                        println!("cargo:rustc-link-search=native={}", out_dir.display());
                        println!("cargo:rustc-link-lib=static=apple_intelligence");
                        println!("cargo:rustc-link-lib=framework=Foundation");
                    }
                  '';
                in ''
                  substituteInPlace src-tauri/build.rs \
                    --replace-fail \
                      'build_apple_intelligence_bridge();' \
                      'build_apple_intelligence_c_stub();'
                  cat ${rsStub} >> src-tauri/build.rs
                ''
              )
              + ''
                # Disable cbindgen in ferrous-opencc (calls cargo metadata which fails in sandbox)
                # Upstream removed this call in v0.3.1+
                substituteInPlace $cargoDepsCopy/ferrous-opencc-0.2.3/build.rs \
                  --replace-fail '.expect("Unable to generate bindings")' '.ok();'
                substituteInPlace $cargoDepsCopy/ferrous-opencc-0.2.3/build.rs \
                  --replace-fail '.write_to_file("opencc.h");' '// skipped'
              '';

              # Bun dependencies: fetched per-package using hashes from .nix/bun.nix.
              # This file is auto-generated by `bunx bun2nix -o .nix/bun.nix` and
              # kept in sync via the postinstall hook in package.json.
              # To regenerate manually: bun scripts/check-nix-deps.ts
              bunDeps = pkgs.bun2nix.fetchBunDeps {
                bunNix = ./.nix/bun.nix;
              };

              nativeBuildInputs = (with pkgs; [
                cargo-tauri.hook
                pkg-config
                bun
                # pkgs.bun2nix (from overlay), not the flake input — `with pkgs;`
                # doesn't shadow function arguments in Nix.
                pkgs.bun2nix.hook # Sets up node_modules from pre-fetched bun cache
                jq
                cmake
                rustPlatform.bindgenHook
              ])
              ++ lib.optionals isLinux (with pkgs; [
                wrapGAppsHook4
                shaderc
              ])
              ++ lib.optionals isDarwin [
                pkgs.darwin.xattr
                # Forward to the real macOS codesign via absolute path
                (pkgs.writeShellScriptBin "codesign" ''exec /usr/bin/codesign "$@"'')
              ];

              # Tests require runtime resources (audio devices, model files, GPU/Vulkan)
              # not available in the Nix build sandbox
              doCheck = false;

              buildInputs = commonNativeDeps pkgs
                ++ lib.optionals isLinux (
                  (with pkgs; [ glib-networking libx11 ])
                  ++ gstPlugins pkgs
                )
                ++ lib.optionals isDarwin [
                  (pkgs.darwinMinVersionHook "14.0")
                ];

              env = commonEnv pkgs // {
                OPENSSL_NO_VENDOR = "1";
              } // lib.optionalAttrs isDarwin {
                MACOSX_DEPLOYMENT_TARGET = "14.0";
                # Link the clang compiler runtime — whisper-rs compiles Objective-C
                # with @available() checks that emit ___isPlatformVersionAtLeast calls
                NIX_LDFLAGS = "-L${pkgs.llvmPackages.compiler-rt-libc}/lib -lclang_rt.osx";
              };

              postFixup = lib.optionalString isDarwin ''
                install_name_tool -add_rpath "${pkgs.onnxruntime}/lib" \
                  $out/Applications/Handy.app/Contents/MacOS/Handy
                # The `tune` pipeline-tuning harness is bundled alongside the app
                # binary and links onnxruntime too (via the shared lib); give it
                # the same rpath so it runs directly from the build output.
                install_name_tool -add_rpath "${pkgs.onnxruntime}/lib" \
                  $out/Applications/Handy.app/Contents/MacOS/tune
              '';

              preFixup = lib.optionalString isLinux ''
                gappsWrapperArgs+=(
                  --set WEBKIT_DISABLE_DMABUF_RENDERER 1
                  --set ALSA_PLUGIN_DIR "${combinedAlsaPlugins}"
                )
              '';

              meta = {
                description = "A free, open source, and extensible speech-to-text application that works completely offline";
                homepage = "https://github.com/cjpais/Handy";
                license = lib.licenses.mit;
                mainProgram = "handy";
                platforms = supportedSystems;
              };
            }
            // lib.optionalAttrs isDarwin {
              # The cargo-tauri.hook standard install handles the Linux .deb, but
              # macOS produces a .app bundle. Extract it and expose a bin/handy
              # symlink so `nix run` / mainProgram resolves.
              # With buildAndTestSubdir = "src-tauri", cargo-tauri.hook builds
              # with the target dir at the source root (not under src-tauri/),
              # so the .app bundle lands at ./target/.../bundle/macos.
              installPhase = ''
                runHook preInstall
                mkdir -p $out/Applications $out/bin
                cp -r target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/bundle/macos/Handy.app $out/Applications/
                ln -s $out/Applications/Handy.app/Contents/MacOS/Handy $out/bin/handy
                # Bundle ffmpeg for the configurable preprocessing pipeline. The
                # app resolves it via BaseDirectory::Resource ("ffmpeg") — the
                # same mechanism used for the silero VAD model. The store-path
                # reference keeps ffmpeg (and its lib closure) as a runtime dep.
                cp ${pkgs.ffmpeg}/bin/ffmpeg $out/Applications/Handy.app/Contents/Resources/ffmpeg
                runHook postInstall
              '';
            }
          );

          default = self.packages.${system}.handy;
        }
      );

      # NixOS module for system-level integration (udev, input group)
      nixosModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/module.nix ];
          programs.handy.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.handy;
        };

      # Home-manager module for per-user service
      homeManagerModules.default =
        { lib, pkgs, ... }:
        {
          imports = [ ./nix/hm-module.nix ];
          services.handy.package = lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.handy;
        };

      # Development shell for building from source
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
          lib = pkgs.lib;
          inherit (pkgs.stdenv) isLinux;
        in
        {
          default = pkgs.mkShell (
            {
              buildInputs = commonNativeDeps pkgs ++ (with pkgs; [
                # Rust toolchain
                rustc
                cargo
                rust-analyzer
                clippy
                # Frontend
                nodejs
                bun
                # Build tools
                cargo-tauri
                pkg-config
                rustPlatform.bindgenHook
                cmake
                # Runtime tool for the configurable preprocessing pipeline; in a
                # packaged build it's bundled into Handy.app/Contents/Resources,
                # but `tauri dev` resolves it from PATH.
                ffmpeg
              ]);

              inherit (commonEnv pkgs)
                ORT_LIB_LOCATION
                ORT_PREFER_DYNAMIC_LINK;

              shellHook = ''
                echo "Handy development environment"
                bun install
                echo "Run 'bun run tauri dev' to start"
              '';
            }
            // lib.optionalAttrs isLinux {
              inherit (commonEnv pkgs) GST_PLUGIN_SYSTEM_PATH_1_0;
              LD_LIBRARY_PATH = "${lib.makeLibraryPath [ pkgs.libayatana-appindicator pkgs.onnxruntime pkgs.vulkan-loader ]}";
              XDG_DATA_DIRS = "${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:${pkgs.hicolor-icon-theme}/share";
            }
          );
        }
      );
    };
}
