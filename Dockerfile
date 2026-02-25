FROM rust:1.83 AS builder

WORKDIR /app
COPY . .
RUN cargo build --release -p opengate

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/opengate /usr/local/bin/opengate

EXPOSE 8080

VOLUME ["/data"]

ENV OPENGATE_DB=/data/opengate.db

ENTRYPOINT ["opengate"]
CMD ["serve", "--port", "8080", "--db", "/data/opengate.db"]
