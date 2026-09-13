//! Embedded single-page console served from the loopback control plane.
//!
//! Static assets are compiled into the binary (`include_str!`) — no build
//! tooling and no external requests. The page talks to the same HTTP API as
//! `curl`: same-origin `fetch` calls carry the `X-Requested-With` CSRF
//! header, so the mutation guard is unchanged.

/// GET `/` — the console page.
pub const INDEX_HTML: &str = include_str!("../assets/ui.html");
/// GET `/ui.css` — console stylesheet.
pub const STYLE_CSS: &str = include_str!("../assets/ui.css");
/// GET `/ui.js` — console logic (polling + mutations).
pub const APP_JS: &str = include_str!("../assets/ui.js");
