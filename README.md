# codex-updater

Ein kleines Rust-Tool, das die lokal installierte `codex`-Version mit dem neuesten stabilen GitHub-Release von OpenAI vergleicht und nur bei Bedarf aktualisiert.

Standardverhalten:

- prüft `https://api.github.com/repos/openai/codex/releases/latest`
- verwendet das Asset `codex-x86_64-unknown-linux-gnu.tar.gz`
- installiert atomisch nach `/usr/local/bin/codex`
- lädt nur herunter, wenn die installierte Version älter ist oder noch nicht existiert
- verifiziert die von GitHub gelieferte `sha256`-Prüfsumme

## Build

```bash
cargo build --release
```

## Nutzung

Nur prüfen:

```bash
./target/release/codex-updater --check-only
```

Prüfen und ggf. installieren:

```bash
sudo ./target/release/codex-updater
```

Optional kann ein GitHub-Token gesetzt werden, um API-Rate-Limits zu entschärfen:

```bash
GITHUB_TOKEN=ghp_xxx sudo ./target/release/codex-updater
```

## systemd beim Boot

Damit das Tool beim Systemstart einmal ausgeführt wird:

1. Das Binary bauen und nach `/usr/local/sbin/codex-updater` installieren:

```bash
cargo build --release
sudo install -m 0755 ./target/release/codex-updater /usr/local/sbin/codex-updater
```

2. Die mitgelieferte Unit-Datei nach `/etc/systemd/system/` kopieren:

```bash
sudo install -m 0644 ./systemd/codex-updater.service /etc/systemd/system/codex-updater.service
```

3. `systemd` neu laden und den Service für den Boot aktivieren:

```bash
sudo systemctl daemon-reload
sudo systemctl enable codex-updater.service
```

4. Optional direkt testen:

```bash
sudo systemctl start codex-updater.service
sudo systemctl status codex-updater.service
```

Logs des aktuellen Boots:

```bash
journalctl -u codex-updater.service -b
```
