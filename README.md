# codex-updater

A small Rust tool that compares the locally installed `codex` version with the latest stable GitHub release from OpenAI and updates it only when necessary.

Default behavior:

- checks `https://api.github.com/repos/openai/codex/releases/latest`
- uses the `codex-x86_64-unknown-linux-gnu.tar.gz` asset
- installs atomically to `/usr/local/bin/codex`
- downloads only when the installed version is older or not present yet
- verifies the `sha256` digest provided by GitHub

## Build

```bash
cargo build --release
```

## Usage

Check only:

```bash
./target/release/codex-updater --check-only
```

Check and install if needed:

```bash
sudo ./target/release/codex-updater
```

Optionally set a GitHub token to reduce API rate-limit pressure:

```bash
GITHUB_TOKEN=ghp_xxx sudo ./target/release/codex-updater
```

Optionally route all GitHub traffic from the updater through a SOCKS5 proxy. Supported schemes are `socks5://` and `socks5h://`:

```bash
sudo ./target/release/codex-updater --socks5-proxy socks5h://127.0.0.1:1080
```

Alternatively via environment variable:

```bash
SOCKS5_PROXY=socks5h://127.0.0.1:1080 sudo ./target/release/codex-updater
```

## systemd At Boot

To run the tool once at system startup:

1. Build the binary and install it to `/usr/local/sbin/codex-updater`:

```bash
cargo build --release
sudo install -m 0755 ./target/release/codex-updater /usr/local/sbin/codex-updater
```

2. Copy the bundled unit file to `/etc/systemd/system/`:

```bash
sudo install -m 0644 ./systemd/codex-updater.service /etc/systemd/system/codex-updater.service
```

3. Reload `systemd` and enable the service for boot:

```bash
sudo systemctl daemon-reload
sudo systemctl enable codex-updater.service
```

4. Optionally test it immediately:

```bash
sudo systemctl start codex-updater.service
sudo systemctl status codex-updater.service
```

Logs for the current boot:

```bash
journalctl -u codex-updater.service -b
```
