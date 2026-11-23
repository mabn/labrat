# labrat in a container: a static (musl) binary on an empty base image.
#
# Locally, this builds labrat from source for your machine's platform:
#
#   docker build -t labrat .
#   docker run --rm -p 8080:8080 -e LABRAT_ROOT_TOKEN=glpat-test-token labrat
#
# Releases (.github/workflows/publish-docker.yml) instead package the binaries of a GitHub Release,
# without compiling anything: put the `labrat-<arch>-unknown-linux-musl.tar.xz` archives in a
# directory and pass it as the `artifacts` build context:
#
#   docker buildx build --build-arg BINARY_SOURCE=prebuilt --build-context artifacts=./artifacts \
#     --platform linux/amd64,linux/arm64 .
#
# Data lives in /data/labrat.sqlite; mount a volume there to keep it, or pass `--db :memory:`. Set
# LABRAT_EXTERNAL_URL to the URL clients use (e.g. http://labrat:8080 for a CI service container).

# `source` (the default) or `prebuilt`: the stage the final image takes /labrat and /data from.
ARG BINARY_SOURCE=source

FROM node:22-alpine AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1-alpine AS source
RUN apk add --no-cache musl-dev ca-certificates
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN LABRAT_SKIP_WEB_BUILD=1 LABRAT_REQUIRE_WEB_UI=1 cargo build --profile dist --locked -p labrat \
    && cp target/dist/labrat /labrat \
    && mkdir -p /data

# Runs on the build machine's platform for every target, so no emulation is needed. Alpine
# already has the CA bundle and an xz-capable tar.
FROM --platform=$BUILDPLATFORM alpine:3 AS prebuilt
ARG TARGETARCH
COPY --from=artifacts labrat-*-unknown-linux-musl.tar.xz /artifacts/
RUN case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=aarch64 ;; \
      *) echo "no labrat release binary for $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && mkdir /unpacked \
    && tar -xJf "/artifacts/labrat-$arch-unknown-linux-musl.tar.xz" -C /unpacked \
    && cp "$(find /unpacked -type f -name labrat)" /labrat \
    && mkdir -p /data

FROM ${BINARY_SOURCE} AS binary

FROM scratch
COPY --from=binary /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=binary /labrat /labrat
COPY --from=binary --chown=65534:65534 /data /data
USER 65534:65534
ENV LABRAT_LISTEN=0.0.0.0:8080 \
    LABRAT_DB=/data/labrat.sqlite
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["/labrat"]
