# syntax=docker/dockerfile:1

FROM node:24-bookworm-slim AS web
WORKDIR /web
COPY web/package.json ./
RUN npm install --no-audit --no-fund
COPY web ./
RUN npm run build

FROM rust:1-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
COPY scripts ./scripts
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && cp target/release/laplace /laplace

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 laplace && mkdir /delta && chown laplace /delta
COPY --from=build /laplace /usr/local/bin/laplace
COPY --from=web /web/dist /srv/web
ENV WEB_DIR=/srv/web LAPLACE_CONFIG=/etc/laplace/flows.toml PORT=8090
USER 10001
# 8090 serves the dashboard and job reports; 9090 serves probes and metrics inside the cluster only
EXPOSE 8090 9090
ENTRYPOINT ["laplace"]
CMD ["serve"]
