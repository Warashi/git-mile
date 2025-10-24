set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just ci

fmt:
    cargo fmt-all

fmt-check:
    cargo fmt --all --check

lint:
    cargo lint

test:
    cargo test-all

ci:
    just fmt-check
    cargo lint
    cargo test-all
