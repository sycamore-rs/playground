{
  description = "Azure CLI";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";
  };

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        # Keep the CLI version aligned with server/Cargo.toml.template and Dockerfile.
        packages = [
          pkgs.nodejs_24
          pkgs.azure-cli
          pkgs.wasm-bindgen-cli_0_2_126
        ];
      };
    };
}
