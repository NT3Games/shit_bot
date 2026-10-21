{
  description = "Shit Bot on Telegram - 屎官";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      fenix,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib;

          craneLib = (crane.mkLib pkgs).overrideToolchain fenix.packages.${system}.minimal.toolchain;
          src = craneLib.cleanCargoSource ./.;

          commonArgs = {
            inherit src;
            strictDeps = true;
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          default = self.packages.${system}.shit-bot;

          shit-bot = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;

              meta = {
                description = "Telegram Bot that records messages to a channel";
                homepage = "https://github.com/NT3Games/shit_bot";
                license = lib.licenses.agpl3Only;
                mainProgram = "shit_bot";
              };
            }
          );
        }
      );

      nixosModules = {
        default = self.nixosModules.shit-bot;

        shit-bot =
          {
            config,
            lib,
            pkgs,
            ...
          }:
          let
            cfg = config.services.shit-bot;
            effectiveConfigFile =
              if cfg.redis.enable then
                pkgs.runCommand "shit-bot-config.yaml"
                  {
                    nativeBuildInputs = [ pkgs.yq-go ];
                    redisUrl = "unix://${config.services.redis.servers.shit-bot.unixSocket}";
                  }
                  ''
                    yq eval '.redis = strenv(redisUrl)' "${cfg.configFile}" > "$out"
                  ''
              else
                cfg.configFile;
          in
          {
            options.services.shit-bot = {
              enable = lib.mkEnableOption "shit_bot Telegram bot service";

              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
                defaultText = lib.literalExpression "inputs.shit-bot.packages.\${system}.default";
                description = "The shit_bot package to use.";
              };

              configFile = lib.mkOption {
                type = lib.types.path;
                example = lib.literalExpression "./config.yaml";
                description = ''
                  Path to the `config.yaml` file.
                  Since the token can be supplied via `environmentFile`, this file no longer
                  needs to contain secrets and can be tracked in your NixOS repository.
                '';
              };

              questionsFile = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                example = lib.literalExpression "./questions.yaml";
                description = "Optional path to an external `questions.yaml` file.";
              };

              environmentFile = lib.mkOption {
                type = lib.types.nullOr lib.types.path;
                default = null;
                example = "/var/lib/shit-bot/env";
                description = ''
                  Environment file (passed to systemd `EnvironmentFile`) containing secrets
                  such as `SHIT_BOT_TOKEN=...`.
                '';
              };

              logLevel = lib.mkOption {
                type = lib.types.str;
                default = "info";
                example = "debug";
                description = "Log level passed to `RUST_LOG`.";
              };

              redis = {
                enable = lib.mkOption {
                  type = lib.types.bool;
                  default = true;
                  description = ''
                    Whether to run a dedicated Redis instance (`redis-shit-bot`) listening on
                    `unix:///run/redis-shit-bot/redis.sock`.
                  '';
                };
              };
            };

            config = lib.mkIf cfg.enable {
              services.redis.servers.shit-bot = lib.mkIf cfg.redis.enable {
                enable = true;
                port = 0;
                unixSocketPerm = 660;
              };

              systemd.services.shit-bot = {
                description = "Shit Bot on Telegram";
                after = [
                  "network-online.target"
                ] ++ lib.optional cfg.redis.enable "redis-shit-bot.service";
                wants = [ "network-online.target" ];
                requires = lib.optional cfg.redis.enable "redis-shit-bot.service";
                wantedBy = [ "multi-user.target" ];

                environment = {
                  RUST_LOG = cfg.logLevel;
                  SHIT_BOT_CONFIG = "${effectiveConfigFile}";
                } // lib.optionalAttrs (cfg.questionsFile != null) {
                  SHIT_BOT_QUESTIONS = "${cfg.questionsFile}";
                };

                serviceConfig = {
                  ExecStart = lib.getExe cfg.package;
                  DynamicUser = true;
                  StateDirectory = "shit-bot";
                  WorkingDirectory = "/var/lib/shit-bot";
                  SupplementaryGroups =
                    lib.optional cfg.redis.enable config.services.redis.servers.shit-bot.group;
                  EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;

                  Restart = "on-failure";
                  RestartSec = 5;

                  # Hardening
                  NoNewPrivileges = true;
                  PrivateTmp = true;
                  ProtectSystem = "strict";
                  ProtectHome = true;
                  PrivateDevices = true;
                  ProtectKernelTunables = true;
                  ProtectKernelModules = true;
                  ProtectControlGroups = true;
                  RestrictSUIDSGID = true;
                };
              };
            };
          };
      };
    };
}
