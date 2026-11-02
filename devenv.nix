{ pkgs, lib, config, ... }:
{
  # Web tests (web/tests/*.test.mjs) require Node 22+; keep it in the base
  # configuration so the backend profile can run them too.
  languages.javascript.enable = true;

  profiles.backend.module =
    { config, ... }:
    {
      languages.rust.enable = true;
      services.postgres = {
        enable = true;
        package = pkgs.postgresql_16;
        listen_addresses = "127.0.0.1";
        port = 5432;
        initialDatabases = [{ name = "twt_cloud"; user = "twt"; pass = "twt-dev"; }];
      };
      env = {
        BIND_ADDR = "127.0.0.1:8080";
        OAUTH_ISSUER_URL = "https://dev-idp.example.com";
        OAUTH_AUDIENCE = "twt-api";
        OAUTH_JWKS_FILE = "${config.git.root}/backend/dev/jwks.json";
        # Per-developer keys from backend/dev/gen-dev-keys.sh; the acceptance
        # covers the static-file risk for local dev only.
        I_ACCEPT_DEV_JWKS = "1";
        # web/ served by `python3 -m http.server 5173 --directory web`
        CORS_ALLOWED_ORIGINS = "http://localhost:5173,http://127.0.0.1:5173";
      };
      env.DATABASE_URL = "postgres://twt:twt-dev@${config.env.PGHOST}:${toString config.env.PGPORT}/twt_cloud";
      env.DEVENV_STATE = lib.mkForce (config.devenv.root + "/.devenv/state");
      processes.api = {
        exec = "cargo run";
        cwd = "${config.git.root}/backend";
        after = [ "devenv:processes:postgres" ];
      };
    };
}
