# labrat in a container: a static (musl) binary on an empty base image.
#
#   docker build -t labrat .
#   docker run --rm -p 8080:8080 -e LABRAT_ROOT_TOKEN=glpat-test-token labrat
#
# Data lives in /data/labrat.sqlite; mount a volume there to keep it, or pass `--db :memory:`. Set
# LABRAT_EXTERNAL_URL to the URL clients use (e.g. http://labrat:8080 for a CI service container).

FROM node:22-alpine AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev ca-certificates
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
RUN LABRAT_SKIP_WEB_BUILD=1 LABRAT_REQUIRE_WEB_UI=1 cargo build --profile dist --locked -p labrat \
    && cp target/dist/labrat /labrat \
    && mkdir -p /data

FROM scratch
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /labrat /labrat
COPY --from=build --chown=65534:65534 /data /data
USER 65534:65534
ENV LABRAT_LISTEN=0.0.0.0:8080 \
    LABRAT_DB=/data/labrat.sqlite
EXPOSE 8080
VOLUME /data
ENTRYPOINT ["/labrat"]
