# syntax=docker/dockerfile:1
FROM lukemathwalker/cargo-chef:latest-rust-1.97.1-alpine@sha256:980619ec8e88608ad967ee0fb697ed3b3e4368acdbac71e5e85ae61f9e2f1a2a AS chef
RUN apk add --no-cache build-base cmake perl pkgconf linux-headers ca-certificates
WORKDIR /app
ENV CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Path-patched upstream crates are part of the dependency recipe.
COPY vendor vendor
RUN cargo chef cook --release --locked --recipe-path recipe.json --package partifactory --bin partifactory
COPY . .
RUN cargo build --release --locked --package partifactory --bin partifactory \
    && mkdir -p /output/data /output/run/pf-render /output/tmp \
    && chmod 1777 /output/tmp \
    && chown -R 65532:65532 /output/data /output/run/pf-render \
    && cp target/release/partifactory /output/partifactory \
    && ln -s /partifactory /output/part

FROM scratch AS runtime
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /output/ /
USER 65532:65532
WORKDIR /data
ENV PF_DATA_DIR=/data
EXPOSE 8080 8081 9090
HEALTHCHECK --interval=10s --timeout=4s --start-period=20s --retries=3 CMD ["/partifactory", "healthcheck"]
ENTRYPOINT ["/partifactory"]
CMD ["serve"]
