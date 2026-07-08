---

### Install

macOS / Linux (Homebrew):

```sh
brew install scootship/tap/scootlens
```

Debian/Ubuntu (apt, amd64/arm64/armhf):

```sh
curl -fsSL https://scootship.github.io/apt-tap/pubkey.gpg | sudo gpg --dearmor -o /usr/share/keyrings/scootship-apt-tap.gpg
echo "deb [signed-by=/usr/share/keyrings/scootship-apt-tap.gpg] https://scootship.github.io/apt-tap stable main" | sudo tee /etc/apt/sources.list.d/scootship-apt-tap.list
sudo apt update
sudo apt install scootlens
```

Manual download: each `scootlens-<tag>-<platform>.tar.gz` asset below ships all
three binaries — `scootlensd` (kernel daemon, Web Console embedded and served
at `/`), `scootctl` (CLI client), `scootlens-mcp` (MCP projection). Verify with
the matching `.sha256` file, then:

```sh
tar -xzf scootlens-<tag>-<platform>.tar.gz
./scootlens/scootlensd --engine mock
```

Linux binaries are statically linked (musl); no runtime dependencies. The
chromium engine attaches to a locally installed Chromium/Chrome
(`SCOOTLENS_CHROMIUM_BIN` overrides discovery).
