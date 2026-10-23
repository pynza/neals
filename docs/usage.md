# Usage

Global: `-y` / `--yes` skips confirmations. `neals --help` for full text.

## Commands

| Command | What it does |
|---------|----------------|
| `neals init` | `devenv init` if needed, then add the `neals` block (no register) |
| `neals register` | Add current directory to the registry |
| `neals list` | Show registered projects |
| `neals unregister <name>` | Remove from registry |
| `neals prune` | Drop ghost entries (missing paths) |
| `neals up <name> [-d]` | Start project; follow process logs unless `-d` |
| `neals down <name>` | Stop project |
| `neals refresh <name>` | Soft re-eval (`--update`, `--hard`) |
| `neals status` | Running projects, PIDs, host endpoints |
| `neals logs <name> [-f]` | Tail logs; `-f` follows all processes |
| `neals bash <name>` | Shell in the project's netns (project must be up) |
| `neals info <name>` | Run `devenv info` for the project |
| `neals exec <name> -- …` | One-shot command in that netns + devenv (literal argv) |
| `neals hosts [show\|apply]` | Host DNS fragment (`sudo neals hosts apply`) |
| `neals doctor` | Check tools, dirs, hosts sync, daemon |
| `neals completions <shell>` | Print completion snippet for shell rc |

## Log follow keys

While attached via `neals up` or `neals logs -f` (no PROCESS):

| Key | Action |
|-----|--------|
| `Ctrl+Q` | Detach; project keeps running |
| `Ctrl+B` | Enter project shell (`neals bash`) |
| `Ctrl+C` or `Ctrl+X` | Stop the project; other follow tabs exit too |

Process lines are prefixed like `devenv | …` (bootstrap / nix build) and
keep flowing after processes start; `be | …` / `fe | …` appear alongside once
per-process logs exist (devenv ≥ 2). `neals logs <name> <process> -f` waits
for those files and shows the merged bootstrap stream until they appear.

## Refresh

`neals refresh <name>` stops the project if needed and re-evaluates the devenv
environment.

| Flag | Effect |
|------|--------|
| (default) | Soft: no lockfile change, no wipe |
| `--update` | Run `devenv update` (modifies `devenv.lock`) before eval |
| `--hard` | After confirm (`-y` to skip), delete `.devenv`, `.neals`, and managed runtime; DB data under `.devenv` is wiped |

`--hard` never removes project source, `devenv.lock`, `.env`, or external volumes.

## Project shell

`neals bash` respects `$SHELL`, runs devenv quietly, and for bash/zsh sets a
short prompt `neals:<project> …`, clears the screen on entry, and leaves the
shell if the project is stopped (e.g. `neals down` from another tab). Use
`neals status` (or the routes printed by `neals up`) for services.

## `exec` vs `bash`

`neals exec <name> -- CMD …` is the **non-interactive** door: cwd is the
project root, argv is passed literally (no shell in between), stdio is
inherited (pipes and TTY apps work), and the command's exit code becomes
`neals`' exit code. Use it from scripts, Makefiles, CI:

```bash
neals exec demo -- redis-cli ping
neals exec demo -- sh -c 'test -f be/package.json && echo ok'
cat dump.sql | neals exec demo -- psql db
```

Your own shell still parses the line, so `&&`, `|` and `>` need the usual
quoting and a shell inside the netns:

```bash
neals exec demo -- bash -lc 'cd be && make migrate'
```

`neals bash <name>` is the **interactive** door (session, prompt, exit when the
project stops) and wins for anything you type by hand: reaching a subdirectory
is `cd be`, not a flag.

## Directories & data

| Path | Role |
|------|------|
| `~/.config/neals/projects.json` | Project registry (name → path) |
| `~/.local/state/neals/<project>.log` | Project stdout/stderr from `devenv up --mode all` |
| `~/.local/state/neals/nealsd.log` | Ad-hoc daemon log |
| `~/.local/state/neals/caddy.json` | Generated Caddy config |
| `~/.local/state/neals/caddy.log` | Caddy log |
| `$XDG_RUNTIME_DIR/neals/` | Ad-hoc IPC socket + per-project runtime |
| `/run/neals/nealsd.sock` | System daemon IPC (if installed) |
| `<project>/.neals/*.sock` | Convenience symlinks to UNIX sockets |

Override IPC with `NEALS_SOCKET`. Override HTTP listen with
`NEALS_CADDY_HTTP_ADDR` (e.g. `127.0.0.1:8080`).
