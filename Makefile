SOUS_DATABASE ?= sous.db
SOUS_ADDR ?= 127.0.0.1:3000
RUST_LOG ?= info

export SOUS_DATABASE SOUS_ADDR RUST_LOG

.PHONY: run build migrate seed reset test test-browser clean

run:
	cargo run

build:
	cargo build --release

migrate:
	cargo run -- migrate

seed: migrate
	sqlite3 $(SOUS_DATABASE) < fixtures/2026-w44.sql

reset:
	rm -f $(SOUS_DATABASE) $(SOUS_DATABASE)-shm $(SOUS_DATABASE)-wal
	$(MAKE) seed

test:
	cargo test

test-browser:
	cargo test --test browser -- --ignored

clean:
	cargo clean
