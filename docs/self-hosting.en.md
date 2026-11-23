**English** | [简体中文](self-hosting.md)

# Self-hosting the rendezvous server and relay

The app uses `wss://p2pcli.aiqji.cn` by default, so self-hosting is optional. To use your own, change it under Settings → Connection.

## Rendezvous server

The rendezvous server only issues device codes, exchanges addresses, and forwards the IPv6 packet-send notice. Terminal data does not pass through it. There are two ways to connect, and the client picks from the address format:

| Form | Channel | Use |
| --- | --- | --- |
| `pubkey@ip:port` or `pubkey@host:port` | UDP (iroh, encrypted, and the server identity is authenticated) | You have a public server |
| `wss://host` | WebSocket (TLS) | A platform that only speaks HTTP, such as Cloudflare Workers, with no server to maintain |

If the two forms point at different servers, the devices cannot look each other up. If one self-hosted server opens both listeners, devices on UDP and devices on WebSocket can look each other up.

### Self-hosted server

One command on a Linux server (with systemd, x86_64 or aarch64):

```sh
curl -fsSL https://raw.githubusercontent.com/ILoveBingLu/P2PCLI/main/crates/server/install.sh | sudo bash
```

The script downloads the published binary, checks the checksum, installs it as a service that starts on boot, and prints the address to enter in the app (`pubkey@ip:port`). After that, allow UDP 7788 in the cloud security group. Running it again upgrades (it replaces the program and leaves the existing service config alone). Arguments go after `bash -s --`, for example `| sudo bash -s -- --port 9000`. `--ws-port` opens a WebSocket listener, `--mirror` adds a proxy prefix when a server in China downloads slowly from GitHub, `--uninstall` removes it, and `--help` lists the rest.

You can also run it directly:

```sh
p2pcli-server --data-dir /var/lib/p2pcli --port 7788 --ws-port 7789 --public-ip <public-ip>
```

At startup it prints the UDP address to enter (`pubkey@ip:port`). The IP can be a hostname that resolves to the server. It is resolved on each connection, so changing DNS is enough when the server's IP changes. `--ws-port` is plain WebSocket. For `wss://`, put a reverse proxy in front (for example Caddy: `rdv.example.com { reverse_proxy 127.0.0.1:7789 }`), or put the hostname behind a CDN such as Cloudflare or Alibaba Cloud ESA. A CDN only forwards HTTP, so a hostname on a CDN can only use the `wss://` form. The benefit is that the server IP stays private and the CDN absorbs attacks.

### Cloudflare Workers

`cloudflare/` is the same protocol as a Workers app, in its own folder, deployed with one command:

```sh
cd cloudflare
./deploy.sh your-domain        # Windows: .\deploy.ps1 your-domain
```

The domain must be hosted on Cloudflare. After deploy, clients use `wss://your-domain`. Details, a local trial, and cost are in [cloudflare/README.md](../cloudflare/README.md).

## Relay

The relay that forwards terminal data when a direct path cannot be opened is separate. The official public iroh relays are very slow from China, or unreachable. The relay is [iroh-relay](https://github.com/n0-computer/iroh), and it needs a server with a public IP:

```sh
iroh-relay --config-path relay.toml   # automatic Let's Encrypt certificate
```

Allow TCP 80/443 (the relay) and UDP 7842 (the relay's address probe). Enter the relay address in settings, or set `P2PCLI_RELAY`.
