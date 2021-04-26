<h1>Discord InfluxDB exporter</h1>

![Grafana Preview](preview.png)

<p align="center">
Export Discord chat metrics to InfluxDB time-series DB
</p>

## Building

Requirements: Go 1.16

```shell
git clone https://github.com/terorie/discord-influx.git
go build -o ./discord-influx .
```

## Usage

Reference: `./discord-influx --help`

**Environment variables**

| Key                   | Description                                  |
| --------------------- | -------------------------------------------- |
| `INFLUXDB_TOKEN`      | InfluxDB API token                           |
| `INFLUXDB_TOKEN_FILE` | Path to a file containing InfluxDB API token |
| `DISCORD_TOKEN`       | Discord API token                            |
| `DISCORD_TOKEN_FILE`  | Path to a file containing Discord API token  |

**Flags**

| Key                           | Description           |
| ----------------------------- | --------------------- |
| `--debug`                     | Enables debug logging |
| `--influxdb-url https://...`  | URL to InfluxDB API   |
| `--influxdb-org org_name`     | InfluxDB org name     |
| `--influxdb-bucket my_bucket` | InfluxDB bucket name  |

### Live mode

Live mode listens for new messages indefinitely and continually stats to InfluxDB.

**Usage**

```shell
export INFLUXDB_TOKEN=xxx
export DISCORD_TOKEN=xxx
./discord-influx live \
  --influxdb-url http://localhost:8086 \
  --influxdb-org my_org \
  --influxdb-bucket my_bucket
```

### History mode

History mode backfills stats of past messages to InfluxDB for the given guilds and channels.

This takes a while because Discord's APIs are heavily rate-limited.

It runs once until the export is complete. This operation is idempotent.

**Targets**

Export targets are given are positional arguments, as either of these two forms:
  - `<guild_id>`: Export all channels in given guild
  - `<guild_id>/<channel_id>`: Export specific channel in guild

So for example `historic 1234 9999/123` exports
  - All channels in guild ID `1234`
  - Channel `123` in guild ID `9999`

**Time frame**

The default time frame spans all messages up to the current system time.

The time frame can be adjusted using flags.

**Flags**

| Key                            | Description                            |
| ------------------------------ | -------------------------------------- |
| `--start <message_id>`         | Export after this message ID           |
| `--start 2006-01-02T15:04:05Z` | Export after this RFC 3339 timestamp   |
| `--stop <message_id>`          | Export before this message ID          |
| `--start 2007-01-02T15:04:05Z` | Export before this RFC 3339 timestamp  |

**Usage**

```shell
export INFLUXDB_TOKEN=xxx
export DISCORD_TOKEN=xxx
./discord-influx historic <org_id> [<org_id> ...] \
  --influxdb-url http://localhost:8086 \
  --influxdb-org my_org \
  --influxdb-bucket my_bucket \
  --start 2006-01-02T15:04:05Z \
  --stop 212490023738540032
```

## Metrics

This exporter requires a bucket with 1ns or 1ms accuracy.

With the following labels:

| Name      | Description               |
| --------- | ------------------------- |
| `guild`   | Guild ID                  |
| `channel` | Channel Name              |
| `user`    | Username `nick#1234`      |
| `emoji`   | Emoji char or reaction ID |

It exports the following time series:

| Name                        | Field   | Labels             |
| --------------------------- | ------- | ------------------ |
| `discord_messages`          | `count` | `guild`, `channel` |
| `discord_user_messages`     | `count` | `guild`, `user`    |
| `discord_message_reactions` | `count` | `guild`, `emoji`   |

## Troubleshooting

### Invalid Discord token

Logs: `{"error": "HTTP 401 Unauthorized, {\"message\": \"401: Unauthorized\", \"code\": 0}", "guild": "xxx"}`

Meaning: Your Discord Bot token is invalid.

### Insufficient Discord permissions

Logs: `{"error": "HTTP 403 Forbidden, {\"message\": \"Missing Access\", \"code\": 50001}", "guild": "xxx"}`

Meaning: Your Discord Bot does not have sufficient permissions.

### Invalid InfluxDB token

Logs: `influxdb2client E! Write error: unauthorized: unauthorized access`

Meaning: Your InfluxDB token is invalid or does not have the required permissions.
