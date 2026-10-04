# nvoc-srv-client

Dependency-free client for the resident `nvoc-srv` control plane, used by the
desktop GUI and TUI to register as control-plane consumers (the same registry
the auto-optimizer, CUDA stressor, and MCP agent use).

It mirrors the Rust `srv-client` crate: plain loopback GET/POST with the
`X-Requested-With: XMLHttpRequest` CSRF header, a `Bearer` local token for
authentication, and a `Session` that opens a consumer session, renews the
lease on a background heartbeat, and closes it on exit so the srv restores
driver `auto`.
