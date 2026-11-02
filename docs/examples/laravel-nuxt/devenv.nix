{ pkgs, lib, ... }: {
  imports = [
    ({ lib, ... }: {
      options.neals = lib.mkOption {
        type = lib.types.attrs;
        default = { };
      };
    })
  ];

  neals = {
    name = "laravel-nuxt";
    services = {
      db.port = 3306;
      be = { port = 8000; proxy = true; };
      fe = { port = 3000; proxy = true; };
      adminer = { port = 8080; proxy = true; };
    };
  };

  languages.php.enable = true;
  languages.php.extensions = [
    "ctype"
    "curl"
    "fileinfo"
    "filter"
    "gd"
    "iconv"
    "intl"
    "mbstring"
    "mysqli"
    "openssl"
    "pdo_mysql"
    "session"
    "tokenizer"
    "zip"
    # "xdebug"
  ];

  # Guest → host loopback (IDE debuggers, etc.):
  # Neals disables host loopback by default. For Xdebug (or any tool that must
  # reach a listener on the host's 127.0.0.1), set on nealsd:
  #   Environment=NEALS_ALLOW_HOST_LOOPBACK=1
  # then restart nealsd and recreate the project (`neals down` / `neals up`).
  # Point the client at 10.0.2.2 (slirp gateway), not 127.0.0.1.
  # See docs/services.md → "Guest → host loopback".
  # languages.php.ini = ''
  #   xdebug.mode=debug
  #   xdebug.client_host=10.0.2.2
  #   xdebug.client_port=9003
  # '';

  languages.javascript.enable = true;

  services.mysql = {
    enable = true;
    package = pkgs.mariadb;
    settings.mysqld = {
      bind-address = "127.0.0.1";
      port = 3306;
    };
    initialDatabases = [{ name = "laravel-nuxt"; }];
    ensureUsers = [{
      name = "dbuser";
      host = "localhost";
      password = "dbpassword";
      ensurePermissions = { "*.*" = "ALL PRIVILEGES"; };
    }];
  };

  services.adminer = {
    enable = true;
    listen = "127.0.0.1:8080";
  };

  processes.be = {
    after = [ "devenv:mysql:configure" ];
    exec = ''
      cd be
      [ -d vendor ] || composer install
      exec php artisan serve --host 127.0.0.1 --port 8000
    '';
  };

  processes.fe.exec = ''
    cd fe
    [ -d node_modules ] || npm install
    exec npm run dev -- --host 127.0.0.1 --port 3000
  '';
}
