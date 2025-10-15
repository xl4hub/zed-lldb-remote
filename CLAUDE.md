# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a **Zed editor debugger extension** that enables remote LLDB debugging via TCP. It wraps `lldb-dap` to allow attaching to remote `lldb-server` instances using `target: "tcp://HOST:PORT"` without requiring a local `pid` or `program` path.

**Target API:** `zed_extension_api = "0.7"` (Rust edition 2024, requires nightly toolchain)

## Build Commands

Build the extension (produces `extension.wasm`):
```bash
cargo +nightly build --release --target wasm32-wasip1
```

The compiled WASM binary is located at:
```
target/wasm32-wasip1/release/lldb_remote_ext.wasm
```

## Development Setup

Ensure Rust nightly with WASM target:
```bash
rustup toolchain install nightly
rustup +nightly target add wasm32-wasip1
```

Install into Zed: **Extensions → Install Dev Extension** → select this directory

## Remote Debugging Workflow

### Starting the Remote Debug Server

On the target machine (or locally for testing), launch the application and attach `lldb-server`:

```bash
$SYSROOT/bin/esync-bridge -h &
pid=$!
echo "pid=$pid"

# start lldb-server in the foreground so Ctrl-C hits it too
lldb-server-20 gdbserver :2345 --attach "$pid"
```

This workflow:
1. Launches the target application in the background
2. Captures its PID
3. Starts `lldb-server` on port 2345, attached to that process
4. The extension then connects via `target: "tcp://127.0.0.1:2345"` in `.zed/debug.json`

### Example Working Configuration

```json
{
  "label": "Attach remote (LLDB-DAP)",
  "adapter": "lldb-remote",
  "request": "attach",
  "target": "tcp://127.0.0.1:2345",
  "program": "/path/to/local/binary",
  "pathMappings": [
    {
      "localRoot": "/local/path",
      "remoteRoot": "/remote/path"
    }
  ],
  "env": {
    "DEBUGINFOD_URLS": "http://your-debuginfod-server:8401"
  },
  "initCommands": [
    "settings set symbols.enable-external-lookup true",
    "settings set target.debug-file-search-paths ~/.cache/llvm-debuginfod/client:~/.cache/debuginfod_client:/usr/lib/debug/.build-id",
    "settings set target.source-map /remote/path /local/path"
  ],
  "attachCommands": [
    "breakpoint set --file /remote/path/to/source.c --line 100",
    "continue"
  ],
  "stopOnEntry": false
}
```

**Important notes:**
- Breakpoints must use **full paths** with the remote path prefix (e.g., `/remote/path/to/source.c`)
- Set breakpoints in `attachCommands` before `continue` - UI breakpoints don't resolve correctly
- `stopOnEntry: false` prevents stopping in libc initialization
- The extension automatically handles SIGSTOP from lldb-server attach

## Architecture

### Extension Entry Point (`src/lib.rs`)

The extension implements `zed::Extension` trait with two key hooks:

1. **`dap_request_kind()`** (line 22-41):
   - Captures user's `.zed/debug.json` configuration
   - Stores raw JSON in `last_config_json` for later use
   - Determines attach vs launch mode from `request` field

2. **`get_dap_binary()`** (line 44-132):
   - Spawns `lldb-dap` with transformed configuration
   - Extracts `tcp://HOST:PORT` from user's `target` field
   - Builds LLDB attach commands: `["gdb-remote HOST:PORT", ...]`
   - Forwards environment variables (e.g., `DEBUGINFOD_URLS`) from debug.json to adapter process
   - Preserves `stopOnEntry`, `initCommands`, and user-provided `attachCommands`
   - **NOTE:** Line 122 hardcodes `"lldb-dap-20"` as the binary name (may need adjustment for different systems)

### Configuration Schema (`debug_adapter_schemas/lldb-remote.json`)

Defines the JSON schema for `.zed/debug.json` entries:
- `target`: Required for TCP attach (format: `tcp://HOST:PORT`)
- `pathMappings`: Maps local to remote source paths
- `env`: Environment variables passed to adapter process
- `initCommands`/`attachCommands`: Custom LLDB commands
- Schema enforces that TCP attach doesn't require `pid` or `program`

### Extension Metadata (`extension.toml`)

- Extension ID: `lldb-remote`
- Registers debug adapter: `debug_adapters.lldb-remote`
- Links to schema at `debug_adapter_schemas/lldb-remote.json`

## Key Implementation Details

**Binary name override:** The extension calls `lldb-dap-20` (src/lib.rs:138). Users may need to symlink or modify this to match their system's binary name (`lldb-dap`, `lldb-vscode`, etc.).

**Configuration transformation:** The extension intercepts user config and transforms it for TCP remote attach:
- User provides: `target: "tcp://HOST:PORT"` and `program: "/path/to/local/binary"`
- Extension generates: `attachCommands: ["target create /path/to/local/binary", "gdb-remote HOST:PORT"]`
- The `target create` before `gdb-remote` is critical - it loads debug symbols from the local binary BEFORE connecting
- This enables symbol resolution and breakpoints for remote debugging

**Symbol resolution:** The extension automatically handles symbol loading by:
1. Running `target create` with the local binary path (from `program` field)
2. Then connecting via `gdb-remote` to the remote process
3. LLDB matches the build-id/UUID between local and remote binaries
4. User's `attachCommands` run after connection for custom setup

**Environment variable forwarding:** Any `env` object in `.zed/debug.json` is passed to the `lldb-dap` process environment, enabling features like debuginfod symbol resolution.

**Remote debugging workflow:** The typical workflow is:
1. Start process on remote machine and attach `lldb-server gdbserver :PORT --attach PID`
2. In Zed, use a debug config with `adapter: "lldb-remote"`, `target: "tcp://HOST:PORT"`, and `program: "/path/to/local/binary"`
3. Set breakpoints in `attachCommands` using full remote paths
4. Start debugging - the extension handles symbol loading and SIGSTOP automatically

## Known Limitations

**Breakpoint Path Resolution**: lldb-dap doesn't properly translate Zed's UI breakpoint requests when using source-map. Breakpoints set through Zed's UI will show as "unverified" and won't hit.

**Workaround**: Set breakpoints in `attachCommands` using the full remote path from debug info:
```json
"attachCommands": [
  "breakpoint set --file /remote/build/path/to/source.c --line 100",
  "continue"
]
```

To find the correct path, use `image lookup -n main -v` in the LLDB console and look at the `CompileUnit: file =` line.

**Why this happens**: The debug info contains full compilation paths (e.g., `/home/builder/project/src/main.c`). While `target.source-map` correctly maps these for viewing source, lldb-dap doesn't apply the same mapping when translating DAP `setBreakpoints` requests from Zed. Short filenames like `main.c` can't be resolved without the full path.

**Variable Editing UI Issue**: When editing variable values in Zed's variables panel, you may see an error `ERROR [project] missing field 'value'`. The variable modification actually succeeds, but Zed's UI fails to parse lldb-dap's response because lldb-dap returns `"result"` while Zed expects `"value"` per the DAP specification.

**Workaround**: Use the LLDB console to verify variable changes:
```
p variable_name
```

This is a bug either in Zed's DAP client (if the spec allows `result`) or in lldb-dap (if it should use `value`). The extension cannot fix this as it doesn't intercept DAP protocol messages.
