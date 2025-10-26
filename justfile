set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just ci

fmt:
    cargo fmt-all

fmt-check:
    cargo fmt --all --check

lint:
    cargo lint

lint-pedantic:
    cargo clippy --workspace --all-targets --all-features -- --cap-lints=warn -W clippy::pedantic -W clippy::nursery -W clippy::cargo

test:
    cargo test-all

ci:
    just fmt-check
    cargo lint
    cargo test-all
