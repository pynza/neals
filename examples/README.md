# Examples

Reference `devenv.nix` snippets that show how to wire [devenv](https://devenv.sh)
projects with `neals.services` / `neals.name`. They are **not** full runnable
apps — copy the pattern into your own tree (and adjust paths, DB names, ports).

| Example | What it shows |
|---------|----------------|
| [laravel-nuxt](laravel-nuxt/devenv.nix) | PHP (Laravel) + JS frontend + MariaDB + Adminer; proxied `be` / `fe` / `adminer`, private `db` |
