# P2PCLI Patch

Source: crates.io `netdev` 0.46.3.
Archive SHA-256: `c3c52c2584961c68f5a6e3356797344061f818c93b8acc9cd6d7e284795e563e`.
Distributed under the upstream MIT license; see `LICENSE`.

Local changes (backported from upstream `netdev` 0.47.0):

- Build the Darwin `SIOCGIFAFLAG_IN6` IPv6 address-flag lookup on macOS only.
  The iOS SDK does not ship `netinet6/in6_var.h`, and `libc` defines
  `in6_ifreq` for macOS only, so 0.46.3 fails to compile for `aarch64-apple-ios`.
- On iOS, IPv6 address flags fall back to the defaults, as on Linux/Android
  without netlink.

Why vendored: iroh 1.4.1 (pinned in the workspace) depends on `netwatch` 0.19.3,
which requires `netdev` 0.46. The mobile FFI crate (`crates/mobile-ffi`) must build
for iOS. Remove this patch once iroh's `netwatch` moves to `netdev` 0.47 or newer.
