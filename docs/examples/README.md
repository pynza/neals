# Examples

Reference `devenv.nix` snippets that show how to wire [devenv](https://devenv.sh)
projects with `neals.services` / `neals.name`. They are **not** full runnable
apps — copy the pattern into your own tree (and adjust paths, DB names, ports).

See also [services.md](../services.md) for the API.

| Example | What it shows |
|---------|----------------|
| [laravel-nuxt](laravel-nuxt/devenv.nix) | Laravel + Nuxt + MariaDB + Adminer; proxied `be` / `fe` / `adminer`, private `db` |
| [laravel-react](laravel-react/devenv.nix) | Laravel + React + MariaDB + Mailpit + Adminer (same Neals shape as a typical full-stack app) |
