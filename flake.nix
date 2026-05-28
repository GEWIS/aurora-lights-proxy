{
  nixConfig = {
    extra-substituters = ["https://nix-community.cachix.org"];
    extra-trusted-public-keys = ["nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCUSeBc="];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    nixpkgs,
    fenix,
    naersk,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [fenix.overlays.default];
      };
      inherit (pkgs) lib;

      winTarget = "x86_64-pc-windows-gnu";
      mingwCC = pkgs.pkgsCross.mingwW64.stdenv.cc;

      devToolchain = pkgs.fenix.combine [
        pkgs.fenix.complete.cargo
        pkgs.fenix.complete.clippy
        pkgs.fenix.complete.rust-analyzer
        pkgs.fenix.complete.rust-src
        pkgs.fenix.complete.rustc
        pkgs.fenix.complete.rustfmt
        pkgs.fenix.targets.${winTarget}.latest.rust-std
      ];

      winToolchain = pkgs.fenix.combine [
        pkgs.fenix.minimal.cargo
        pkgs.fenix.minimal.rustc
        pkgs.fenix.targets.${winTarget}.latest.rust-std
      ];

      naerskWin = naersk.lib.${system}.override {
        cargo = winToolchain;
        rustc = winToolchain;
      };

      crossEnv = let
        pthreads = pkgs.pkgsCross.mingwW64.windows.pthreads;
        winGCC = lib.meta.getExe' mingwCC "x86_64-w64-mingw32-gcc";
        winAR = lib.meta.getExe' mingwCC "x86_64-w64-mingw32-ar";
      in {
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER = winGCC;
        CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS = "-L ${pthreads}/lib";
        CC_x86_64_pc_windows_gnu = winGCC;
        AR_x86_64_pc_windows_gnu = winAR;
      };
    in {
      packages.windows = naerskWin.buildPackage ({
          src = ./.;
          CARGO_BUILD_TARGET = winTarget;
          nativeBuildInputs = [mingwCC];
        }
        // crossEnv);

      devShells.default = pkgs.mkShell ({
          packages = [
            devToolchain
            pkgs.bacon
            mingwCC
          ];
        }
        // crossEnv);
    });
}
