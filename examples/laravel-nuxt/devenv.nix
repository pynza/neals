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
  ];

  languages.javascript.enable = true;

  services.mysql = {
    enable = true;
    package = pkgs.mariadb;
    settings.mysqld = {
      bind-address = "127.0.0.1";
      port = 3306;
    };
    initialDatabases = [{ name = "mcr-easy"; }];
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

  processes.be.exec = ''
    cd be
    [ -d vendor ] || composer install
    exec php artisan serve --host 127.0.0.1 --port 8000
  '';

  processes.fe.exec = ''
    cd fe
    [ -d node_modules ] || npm install
    exec npm run dev -- --host 127.0.0.1 --port 3000
  '';
}
