//! Core services. Port of xterm.js `src/common/services/`.
//!
//! xterm.js wires these through a DI container (`InstantiationService`); the port uses plain
//! owned structs on the terminal instead. Port order: `OptionsService`, `UnicodeService`
//! (+ `@xterm/addon-unicode11` 0.9.0 tables), `CharsetService`, `BufferService`, `CoreService`,
//! `CoreMouseService`, `OscLinkService`.
