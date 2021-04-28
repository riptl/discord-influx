FROM golang:1.16-alpine AS builder
WORKDIR /app
COPY . .
RUN \
   --mount=type=cache,target=/go/pkg \
   --mount=type=cache,target=/root/.cache/go-build \
   CGO_ENABLED=0 go build -o discord-influx .

FROM scratch
COPY --from=builder /app/discord-influx /
ENTRYPOINT ["/discord-influx"]
LABEL org.opencontainers.image.source="https://github.com/terorie/discord-influx"
