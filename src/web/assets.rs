//! Static web assets embedded in the binary.

/// Main HTML page.
pub const INDEX_HTML: &str = include_str!("../../web/index.html");

/// Pico.css v2 – classless CSS framework.
pub const PICO_CSS: &str = include_str!("../../web/static/pico.min.css");

/// htmx 1.9.x – hypermedia-driven frontend.
pub const HTMX_JS: &str = include_str!("../../web/static/htmx.min.js");

/// Alpine.js 3.x – lightweight reactive framework for modal interactions.
pub const ALPINE_JS: &str = include_str!("../../web/static/alpine.min.js");
