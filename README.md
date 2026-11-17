# Sous

Sous is a small, single-household web service that shows a weekly meal plan. It reads the plan from a SQLite
database and serves it as a mobile-friendly page: daily meals with nutrition summaries, shopping lists and prep
sessions. It also has a week archive by year, a JSON read API and an MCP endpoint for AI assistants.

The web pages and the API are read-only, except the household profile (preferences such as who eats, diets and
kitchen equipment), which can be edited on the `/profile` page. To add or change plans, connect an assistant over
[MCP](#mcp), or edit the database with another tool, such as `sqlite3` or a script. Changes show up on the next page
load. The app has no accounts or authentication.

## Install

You need a Rust toolchain with edition 2024 support (Rust 1.85 or later) and `sqlite3` to load data. The frontend
is compiled during the Cargo build and embedded in the binary, so you don't need Node.

```sh
git clone <repository-url> sous
cd sous
cargo install --path .
```

This builds the `sous` binary and puts it on your `PATH`.

## Usage

Start the server:

```sh
sous            # or: make run
```

On startup Sous applies any pending migrations to the database, creating it if needed. Then it listens on
`http://127.0.0.1:3000`. To apply migrations without starting the server, run `sous migrate` (or `make migrate`).

Configure it with environment variables:

| Variable | Default | Purpose |
| --- | --- | --- |
| `SOUS_DATABASE` | `sous.db` | Path to the SQLite database |
| `SOUS_ADDR` | `127.0.0.1:3000` | Listen address |
| `APP_TIMEZONE` | `Europe/Berlin` | Timezone used to work out today and the current week |
| `RUST_LOG` | unset: errors only (`make` sets `info`) | Log level, e.g. `info` or `sous=debug` |
| `SOUS_MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,::1` | Comma-separated `Host` values the MCP endpoint accepts |
| `SOUS_MCP_ALLOWED_ORIGINS` | unset: none | Comma-separated `Origin` values the MCP endpoint accepts; requests with any other `Origin` are rejected |

### Pages and API

| Route | Shows |
| --- | --- |
| `/` | The current ISO week |
| `/<year>/<week>` | A specific ISO week, e.g. `/2026/44` |
| `/<date>` | One day of its week, e.g. `/2026-10-28` |
| `/<year>` | The stored weeks of one ISO week-year, newest first, e.g. `/2026` |
| `/api/<year>/<week>` | The full week as JSON |
| `/api/<date>` | The full week containing that date as JSON |
| `/api/<year>` | The year listing as JSON |

Weeks use ISO 8601 numbering (Monday to Sunday). A plan page shows one day at a time, with links to the previous
and next day, a Today link when you are on another date, and a month calendar that links every date and marks the days with shopping lists or prep sessions. The pages follow the [design system](docs/design-system.md), after Kandinsky's Bauhaus years. They have a light and a dark theme: pick Light, Dark or System at the foot of any page. System follows your device's color-scheme preference. The API is described in the [OpenAPI document](docs/openapi.json).

### MCP

Sous also serves a [Model Context Protocol](https://modelcontextprotocol.io) endpoint at
`http://127.0.0.1:3000/mcp`, so an assistant can plan weeks, recall earlier recipes and fetch today's shopping
lists. To connect Claude Code:

```sh
claude mcp add --transport http sous http://127.0.0.1:3000/mcp
```

Then ask it, for example, "What casserole did we have three weeks ago, and how do I make it?" or "What's on today's
shopping list?"

| Tool | Does |
| --- | --- |
| `get_context` | Today's date, the timezone, the current week and the household profile |
| `list_weeks` | Stored weeks in a date range |
| `get_week` | One complete week and its revision |
| `search_meals` | Meals whose name or description contains some text |
| `get_meal` | One meal with its recipe and date |
| `get_shopping_lists` | The shopping lists for one day, today by default |
| `save_week` | Creates or replaces one complete week |
| `update_profile_section` | Replaces the text of one household profile section |

`save_week` and `update_profile_section` are the tools that write. `save_week` replaces the whole week in one
transaction, and it fails with a conflict if the week changed since the assistant read it. `update_profile_section`
does the same for one profile section.

There is no authentication: keep Sous on the loopback address, or put an authenticated layer in front of it. The
[MCP guide](docs/mcp.md) covers every tool, the save semantics and worked examples.

### Adding data

The schema is defined in the [migrations](migrations) and documented in the
[technical design](docs/technical-design.md). The [fixtures](fixtures) hold a complete example week plus two
earlier weeks for trying recipe recall. To load them into a fresh database:

```sh
make reset      # deletes sous.db, migrates, and loads the fixtures
make run
```

Then open <http://127.0.0.1:3000/2026/44>.

## Docker

The image runs Sous from `scratch`: one statically linked binary, an empty `/data` for the database and an empty
`/tmp`. Build it from a checkout and run it with a named volume:

```sh
docker build -t sous .
docker run -d --name sous -p 3000:3000 -v sous-data:/data sous
```

Then open <http://localhost:3000>. Sous creates `/data/sous.db` on first start and keeps it in the volume. The
process runs as UID 65532, and a new named volume takes the ownership of `/data`. `docker stop sous` shuts the server
down gracefully. To apply migrations without starting the server:

```sh
docker run --rm -v sous-data:/data sous migrate
```

The image sets these defaults. Override any of them with `-e`; `APP_TIMEZONE` and the MCP variables keep the defaults
from the table above.

| Variable | Image default |
| --- | --- |
| `SOUS_ADDR` | `0.0.0.0:3000` |
| `SOUS_DATABASE` | `/data/sous.db` |
| `RUST_LOG` | `info` |

Tagged releases are published for linux/amd64 and linux/arm64 to the GitHub Container Registry as
`ghcr.io/<owner>/<repo>`, where `<owner>/<repo>` is the lowercase name of the GitHub repository. Version `X.Y.Z` is
available as `X.Y.Z`, `X.Y` and `latest`. Pre-releases get only their own version tag. Use the same `docker run`
command with that name in place of `sous`.

A bind mount replaces the volume's ownership handling, so the host directory must be writable by UID 65532, or you
must run the container with a matching `--user`, for example `--user "$(id -u):$(id -g)"`.

Behind a reverse proxy, set `SOUS_MCP_ALLOWED_HOSTS` to the public host name, for example
`-e SOUS_MCP_ALLOWED_HOSTS=sous.example.com`. Otherwise the MCP endpoint rejects the proxied requests. As elsewhere,
Sous has no authentication.

The image has no shell or `sqlite3`, so seed the database from the host. Create it in a directory on the host, stop
any server using it, and mount that directory:

```sh
mkdir data
SOUS_DATABASE=data/sous.db make reset
docker run -d -p 3000:3000 --user "$(id -u):$(id -g)" -v "$PWD/data:/data" sous
```

There is no `HEALTHCHECK`, because the image has no HTTP client. Orchestrators should probe `GET /`.

## Development

```sh
make test           # cargo test
make test-browser   # browser tests; need Chrome or Chromium
```

GitHub Actions runs the format check, clippy, the tests and the browser tests on every pull request and every push
to `main`.

The browser tests find Chrome on your `PATH` or in its standard install location. To use a different browser
binary, set `CHROME` to its path.

See the [feature tracker](docs/README.md) for specs.
