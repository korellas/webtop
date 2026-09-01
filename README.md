# Webtop

Webtop is a single-binary macOS system-monitor dashboard for Apple Silicon.
It embeds a React dashboard in a Rust server and presents live CPU, GPU,
memory, power, network, disk, process, and service information in one local
web interface.

## What it does

- Streams current system metrics over WebSocket and keeps recent history in
  SQLite.
- Shows synchronized charts, process details, storage growth, and network
  history.
- Reads an optional service manifest to display declared services and their
  health without hard-coding a particular stack.
- Builds the frontend into the Rust binary, so deployment is one executable.

## Build and run

Requirements: macOS, Rust, Node.js, and npm.

```sh
./build.sh
./target/release/webtop --port 7890
```

For development, run `cargo run -- --port 7890` in one terminal and
`npm run dev` from `frontend/` in another.

## Security note

Webtop listens on all network interfaces by default. Treat it as a trusted-LAN
tool: do not expose it directly to the public internet. Mutating service and
folder-scan requests require a custom header to prevent drive-by browser
requests; that header is not authentication. Use a firewall or a trusted
network boundary when running the dashboard.

## License

Licensed under the [Apache License 2.0](LICENSE). See [CREDITS.md](CREDITS.md)
for artwork history.
