# Repository automation via just
set shell := ["bash", "-euo", "pipefail", "-c"]

cargo_workspace_flags := "--workspace --all-features"

default:
    @just --list

fmt:
    cargo fmt

alias format := fmt

build:
    cargo build {{cargo_workspace_flags}}

test:
    cargo test {{cargo_workspace_flags}}

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
