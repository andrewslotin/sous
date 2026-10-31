# Sous

Sous is a small, single-household web service that shows a weekly meal plan. It reads the plan from a SQLite
database and serves it as a mobile-friendly page: daily meals with nutrition summaries, shopping lists, prep
sessions and strategy cards. It also has a week archive by year, a JSON read API and an MCP endpoint for AI assistants.

The web pages and the API are read-only. To add or change plans, connect an assistant over [MCP](#mcp), or edit the
database with another tool, such as `sqlite3` or a script. Changes show up on the next page load. The app has no
accounts or authentication.

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
| `/<year>` | The stored weeks of one ISO week-year, newest first, e.g. `/2026` |
| `/api/weeks/<year>/<week>` | The full week as JSON |

Weeks use ISO 8601 numbering (Monday to Sunday). The API is described in the [OpenAPI document](docs/openapi.json).

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
| `get_context` | Today's date, the timezone and the current week |
| `list_weeks` | Stored weeks in a date range |
| `get_week` | One complete week and its revision |
| `search_meals` | Meals whose name or description contains some text |
| `get_meal` | One meal with its recipe and date |
| `get_shopping_lists` | The shopping lists for one day, today by default |
| `save_week` | Creates or replaces one complete week |

`save_week` is the only tool that writes. It replaces the whole week in one transaction, and it fails with a conflict
if the week changed since the assistant read it.

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

## Development

```sh
make test           # cargo test
make test-browser   # browser tests; need Chrome or Chromium
```

The browser tests find Chrome on your `PATH` or in its standard install location. To use a different browser
binary, set `CHROME` to its path.

See the [feature tracker](docs/README.md) for specs.
