# Sous

Sous is a small, single-household web service that shows a weekly meal plan. It reads the plan from a SQLite
database and serves it as a mobile-friendly page: daily meals with nutrition summaries, shopping lists, prep
sessions and strategy cards. It also has a week archive and a JSON read API.

Sous only displays data. You fill the database with another tool, such as `sqlite3` or a script. Changes show up on
the next page load. The app has no accounts or authentication.

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

### Pages and API

| Route | Shows |
| --- | --- |
| `/` | The current ISO week |
| `/<year>/<week>` | A specific ISO week, e.g. `/2026/44` |
| `/weeks` | All stored weeks, newest first |
| `/api/weeks/<year>/<week>` | The full week as JSON |

Weeks use ISO 8601 numbering (Monday to Sunday). The API is described in the [OpenAPI document](docs/openapi.json).

### Adding data

The schema is defined in the [migrations](migrations) and documented in the
[technical design](docs/technical-design.md). The [fixtures](fixtures) are a complete example week.
To load them into a fresh database:

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
