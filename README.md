# Aurora Lights Proxy

[![CI](https://github.com/GEWIS/aurora-lights-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/GEWIS/aurora-lights-proxy/actions/workflows/ci.yml)
[![Release](https://github.com/GEWIS/aurora-lights-proxy/actions/workflows/release.yml/badge.svg)](https://github.com/GEWIS/aurora-lights-proxy/actions/workflows/release.yml)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)
[![Dependabot](https://img.shields.io/badge/Dependabot-enabled-025E8C?logo=dependabot)](.github/dependabot.yml)

A blazing fast Rust proxy that forwards DMX packets from the [Aurora core](https://github.com/GEWIS/narrowcasting-core) to an Art-Net controller over UDP.

The Art-Net controller sits on a link-local interface that can't see the rest of the network, so a host machine acts as the bridge. It connects to Aurora core over Socket.IO, takes DMX frames as they arrive, and re-broadcasts them as Art-Net packets to the controller.

## Prerequisites

- Rust 1.88 or newer (`rustup` recommended)
- An Art-Net controller (for example a Showtec NET-2/3 Pocket) reachable at a known IP; the default is `169.254.0.2` on a direct link-local connection
- A running Aurora core with an API key

## Getting started

```bash
git clone https://github.com/GEWIS/aurora-lights-proxy.git
cd aurora-lights-proxy
cp .env.example .env
# fill in URL and API_KEY in .env

cargo run --release
```

A release build produces a stripped binary at `target/release/aurora-lights-proxy`. Copy it to the host machine and run it next to the `.env` file.

### Cross-compiling

To target a different host (for example a Raspberry Pi):

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

## Configuration

All configuration comes from environment variables. `.env` is loaded automatically on startup.

| Variable      | Required | Default         | Description                                                    |
| ------------- | -------- | --------------- | -------------------------------------------------------------- |
| `URL`         | yes      | --              | Base URL of the Aurora core, e.g. `http://localhost:3000`      |
| `API_KEY`     | yes      | --              | API key used to authenticate at `POST {URL}/api/auth/key`      |
| `LOG_LEVEL`   | no       | `info`          | `tracing` filter directive (e.g. `info`, `debug`, `aurora_lights_proxy=trace`) |
| `TARGET_IP`   | no       | `169.254.0.2`   | IPv4 address of the Art-Net controller                         |
| `UNIVERSE`    | no       | `0`             | DMX universe (0-32767)                                         |
| `PACKET_SIZE` | no       | `512`           | Bytes per Art-Net frame (2-512, even)                          |
| `FPS`         | no       | `40`            | Art-Net send rate in frames per second                         |

## How it works

```
Aurora core <--Socket.IO--> aurora-lights-proxy <--UDP/Art-Net--> controller --> fixtures
```

1. POST to `{URL}/api/auth/key` and capture the `connect.sid` cookie.
2. Open two Socket.IO connections: `/` for lifecycle and status, `/lights` for DMX traffic.
3. On every `dmx_packet` event, clamp channels to `0..=255`, pad or truncate to `PACKET_SIZE`, and update the in-memory DMX buffer.
4. A background thread re-broadcasts the current buffer at `FPS` Hz on UDP port `6454`.
5. Every five seconds, emit `status:update` with uptime, system clock, and the latest round-trip latency.

When the core disconnects or the process is interrupted, the proxy blacks out the universe and stops sending, so fixtures don't stay stuck on the last frame.

### Recovery

The proxy is built to ride through outages on its own. A manual restart should never be necessary.

- Socket.IO reconnects automatically with a 1-60s exponential backoff (handled by `rust_socketio` under the hood). The proxy logs the disconnect, blacks out the universe, and waits for the reconnect to come through.
- If a namespace stays disconnected for more than 45 seconds, the proxy tears down both clients, re-authenticates against `/api/auth/key`, and reopens the connections from scratch. This recovers from expired cookies and "stuck reconnect" states.
- The outer retry loop uses 1, 2, 4, 8, 16, 32, 60s backoff (capped at 60s). The counter resets after 60s of continuous uptime so a long-running session that hiccups once doesn't get penalised.
- Art-Net is fire-and-forget UDP, so we can't detect a powered-off controller from the application layer; the sender keeps emitting frames and they resume reaching the controller as soon as it's back. Local interface errors (`ENETUNREACH`, `EHOSTUNREACH`) are logged but never crash the sender thread.

## Testing

```bash
cargo test            # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Unit tests cover DMX clamp/pad behaviour and Art-Net frame encoding. Integration tests in [`tests/`](tests/) drive the public API and verify the on-wire bytes through a loopback UDP listener.

## Project layout

```
src/
  artnet.rs     UDP sender + 40 Hz broadcast loop
  config.rs     Environment-based configuration
  packet.rs     Pure DMX clamping and Art-Net frame builder
  lib.rs        Module exports for tests
  main.rs       Socket.IO wiring, auth, lifecycle, status loop
tests/
  end_to_end.rs Public API integration tests
```

