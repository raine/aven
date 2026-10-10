# syntax=docker/dockerfile:1
FROM rust:1.99.0-alpine3.23@sha256:42c2519fdf75d9e34cc61a1aaf34b1a684da9bfd65fe5daefef98d92a801b001 AS build
RUN apk add --no-cache build-base cmake perl git
WORKDIR /src
COPY . .
ENV SQLX_OFFLINE=true
RUN cargo build --release --locked --bin aven

FROM alpine:3.23@sha256:85fe1e81d6758c208f3e1eed4338a1997e19d4be002d4dd32d3100c9a8c010a0 AS runtime
RUN addgroup -g 65532 aven && adduser -D -H -u 65532 -G aven aven \
    && mkdir /data && chown 65532:65532 /data && chmod 700 /data
ENV STATE_DIRECTORY=/data
USER 65532:65532
EXPOSE 3746
STOPSIGNAL SIGTERM
LABEL org.opencontainers.image.source="https://github.com/raine/aven" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.description="Aven encrypted sync server"
ENTRYPOINT ["/usr/local/bin/aven"]
CMD ["server", "--bind", "0.0.0.0:3746", "--unsafe-public-bind"]

FROM runtime AS release
ARG TARGETARCH
COPY --from=release-bin --chmod=755 /linux-${TARGETARCH}/aven /usr/local/bin/aven

FROM runtime AS source
COPY --from=build /src/target/release/aven /usr/local/bin/aven
