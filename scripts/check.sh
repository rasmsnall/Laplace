#!/usr/bin/env bash
# the pre-merge checks from SECURITY.md. tools that are not installed locally run in docker.
set -euo pipefail
cd "$(dirname "$0")/.."
# keeps git bash on windows from rewriting the paths given to docker
export MSYS_NO_PATHCONV=1

step() { printf '\n== %s\n' "$1"; }

step "secret scan"
docker run --rm -v "$PWD:/repo" zricethezav/gitleaks:latest dir /repo --no-banner --redact \
    --config /repo/.gitleaks.toml

step "format"
cargo fmt --check

step "lint"
cargo clippy --all-targets -- -D warnings

step "tests"
cargo test

step "rust advisories"
docker run --rm -v "$PWD:/repo" -w /repo rust:1-bookworm \
    sh -c "cargo install cargo-audit --locked --quiet && cargo audit"

step "dashboard advisories and build"
docker run --rm -v "$PWD/web:/web" -w /web node:24-bookworm-slim \
    sh -c "npm install --no-audit --no-fund --silent && npm audit --audit-level=high && npm run build"

step "flows.toml and helm chart"
cargo run --quiet -- check flows.toml
docker run --rm -v "$PWD/deploy/helm:/charts" alpine/helm:latest lint /charts/laplace
docker run --rm -v "$PWD/deploy/helm:/charts" alpine/helm:latest template ci /charts/laplace \
    --set ingress.enabled=true --set networkPolicy.enabled=true > target/laplace-chart.yaml
docker run --rm -v "$PWD/target:/work" ghcr.io/yannh/kubeconform:latest -strict -summary /work/laplace-chart.yaml

printf '\nall checks passed\n'
