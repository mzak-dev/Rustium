# Scripts are Luau, run through mlua

Scripts use Luau via `mlua` (vendored) because Luau ships a sandbox and resource limits, which matters once MCP clients may run scripts, and Lua is familiar to artists. Scripts reach the Scene only through the Command API. Rhai (pure Rust, smaller) and WASM (any language, heavy runtime) were considered; the cost we accept is a C++ build step for the vendored Luau, isolated in `rustium-script`.
