# P2PCLI Patch

Source: crates.io `iroh-mdns-address-lookup` 0.6.1.
Archive SHA-256: `c7e00b132891c9cfad726fb437b62315e722949d409717b6c7dcb535e9f24112`.
Upstream commit: `0f5f547cc65514e4537d4e66f776319ea8e30a2e`
(`n0-computer/iroh-address-lookups`, `iroh-mdns-address-lookup` directory).
Distributed under the upstream MIT license option; see `LICENSE-MIT`.

Local changes:

- Expose explicit IPv4 multicast interfaces through the builder.
- Forward runtime interface changes to swarm-discovery without replacing the
  discovery cache, subscriptions, or the iroh endpoint.
- Require swarm-discovery 0.6.5 or newer in the 0.6 series for its per-interface
  multicast send and receive behavior.

P2PCLI supplies interfaces for both the persistent advertiser and scan-only
probes, refreshing the persistent interface set on each scan. The desktop
already scans every 30 seconds and on manual refresh. IPv6 behavior and the
default behavior for callers that do not select interfaces remain unchanged.

Do not edit Cargo's registry cache. Remove this patch only after an upstream
release exposes equivalent interface selection and refresh APIs.

Validation commands (these compile code; run explicitly when appropriate):

```sh
cargo test -p p2pcli-core lan::interfaces::tests
cargo test -p iroh-mdns-address-lookup interfaces
cargo test -p iroh-mdns-address-lookup runtime_interface_updates
cargo test -p iroh-mdns-address-lookup discovery_uses_an_explicit_interface -- --ignored
```

The last test uses local multicast sockets and verifies runtime interface
selection, discovery, and preservation of device metadata on loopback.
