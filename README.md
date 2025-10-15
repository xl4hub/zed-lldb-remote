# LLDB Remote Debugging Extension for Zed

Remote LLDB debugging via TCP for Zed editor. This extension enables connecting to `lldb-server` instances for embedded systems, cross-platform, and remote debugging scenarios.

## Features

- **TCP Remote Attach**: Connect to `lldb-server` via `tcp://HOST:PORT` without requiring local `pid` or `program` paths
- **Source Path Mapping**: Automatic source-map generation from `pathMappings` configuration
- **Variable Expansion**: Portable configurations with `${HOME}` and `${USER}` variable support
- **Symbol Resolution**: Integrated debuginfod support for automatic debug symbol fetching
- **Custom LLDB Commands**: Support for `initCommands` and `attachCommands` to customize debugging workflow

## Requirements

- **lldb-dap** (LLDB's Debug Adapter Protocol implementation)
  - Ubuntu/Debian: `apt install lldb`
  - The extension expects `lldb-dap-20` by default (create a symlink if needed)
- **lldb-server** running on the target machine

## Installation

### Recommended: Dev Mode Installation

For personal or team use, dev mode is the easiest approach:

1. Clone this repository:
   ```bash
   git clone https://github.com/xl4hub/zed-lldb-remote.git
   cd zed-lldb-remote
   ```

2. Build the extension:
   ```bash
   rustup toolchain install nightly
   rustup +nightly target add wasm32-wasip1
   cargo +nightly build --release --target wasm32-wasip1
   ```

3. In Zed: **Extensions → Install Dev Extension**
4. Select the cloned directory

**Note**: Dev mode installation is persistent across Zed updates. You only need to rebuild if the extension API changes.

### From Zed Extensions Registry (Future)

Once published to the official registry:

1. Open Zed
2. Press `cmd-shift-p` (macOS) or `ctrl-shift-p` (Linux/Windows)
3. Search for "extensions"
4. Find "LLDB Remote (TCP Attach)" and click Install

## Quick Start

### 1. Start the Remote Debug Server

On your target machine (or locally for testing):

```bash
# Launch your application
./your-application &
pid=$!

# Attach lldb-server on port 2345
lldb-server gdbserver :2345 --attach "$pid"
```

### 2. Configure Zed Debug

Create `.zed/debug.json` in your project:

```json
[
  {
    "label": "Attach to Remote",
    "adapter": "lldb-remote",
    "request": "attach",
    "target": "tcp://127.0.0.1:2345",
    "program": "${ZED_WORKTREE_ROOT}/build/your-application",
    "pathMappings": [
      {
        "localRoot": "${ZED_WORKTREE_ROOT}",
        "remoteRoot": "/remote/build/path"
      }
    ],
    "stopOnEntry": true
  }
]
```

### 3. Start Debugging

1. Press `F5` or use the Debug menu
2. Select your configuration
3. Start debugging!

## Configuration Reference

### Required Fields

| Field | Type | Description |
|-------|------|-------------|
| `adapter` | string | Must be `"lldb-remote"` |
| `request` | string | Use `"attach"` for remote debugging |
| `target` | string | TCP address in format `tcp://HOST:PORT` |

### Optional Fields

| Field | Type | Description |
|-------|------|-------------|
| `program` | string | Path to local binary (for symbol loading). Supports `${HOME}` and `${USER}` variables. |
| `pathMappings` | array | Maps remote source paths to local paths |
| `pathMappings[].localRoot` | string | Local source directory. Supports `${ZED_WORKTREE_ROOT}`, `${HOME}`, `${USER}` |
| `pathMappings[].remoteRoot` | string | Remote source directory. Supports `${HOME}`, `${USER}` |
| `env` | object | Environment variables for lldb-dap process (e.g., `DEBUGINFOD_URLS`) |
| `initCommands` | array | LLDB commands run during initialization |
| `attachCommands` | array | LLDB commands run after attaching to target |
| `stopOnEntry` | boolean | Whether to stop at the entry point (default: false) |

### Variable Expansion

The extension supports these variables in paths:

- `${ZED_WORKTREE_ROOT}` - Root directory of the current worktree
- `${HOME}` - User's home directory
- `${USER}` - Username extracted from home path

## Advanced Examples

### Embedded System Debugging with Symbol Server

```json
{
  "label": "Attach to Embedded Target",
  "adapter": "lldb-remote",
  "request": "attach",
  "target": "tcp://192.168.1.100:2345",
  "program": "${HOME}/projects/embedded/build/firmware.elf",
  "pathMappings": [
    {
      "localRoot": "${HOME}/projects/embedded/src",
      "remoteRoot": "/build/workspace/src"
    }
  ],
  "env": {
    "DEBUGINFOD_URLS": "http://debuginfod.example.com:8080"
  },
  "initCommands": [
    "settings set symbols.enable-external-lookup true"
  ],
  "stopOnEntry": false
}
```

### Wait for Breakpoint Pattern

For applications that loop waiting for debugger attachment:

```json
{
  "label": "Attach and Wait for Breakpoint",
  "adapter": "lldb-remote",
  "request": "attach",
  "target": "tcp://127.0.0.1:2345",
  "program": "${ZED_WORKTREE_ROOT}/build/app",
  "pathMappings": [
    {
      "localRoot": "${ZED_WORKTREE_ROOT}",
      "remoteRoot": "${HOME}/builder/workspace"
    }
  ],
  "initCommands": [
    "command script import ${ZED_WORKTREE_ROOT}/.zed/debug.py"
  ],
  "attachCommands": [
    "breakpoint set --name wait_for_debugger",
    "continue",
    "wait-stop 2",
    "expr debugger_attached = 1"
  ],
  "stopOnEntry": true
}
```

Custom `debug.py` for `wait-stop` command:

```python
import lldb
import time

def wait_stop_cmd(dbg, arg, ctx, res, _):
    timeout = float(arg) if arg else 10.0
    proc = ctx.GetProcess() or dbg.GetSelectedTarget().process
    if not proc or not proc.IsValid():
        res.SetError("no valid process")
        return

    end = time.time() + timeout
    while proc.GetState() not in (lldb.eStateStopped, lldb.eStateExited) and time.time() < end:
        lldb.SBHostOS.Sleep(lldb.TimeValue(0, 200_000_000))  # 200ms

    if proc.GetState() != lldb.eStateStopped:
        res.SetError("timeout waiting for stop")

def __lldb_init_module(dbg, _):
    dbg.HandleCommand("command script add -f debug.wait_stop_cmd wait-stop")
```

## How It Works

The extension transforms your debug configuration for `lldb-dap`:

1. **Captures Configuration**: When you start debugging, the extension receives your `.zed/debug.json` config
2. **Extracts TCP Target**: Parses `target: "tcp://HOST:PORT"`
3. **Builds Attach Commands**:
   - `target create <program>` - Loads symbols from local binary
   - `gdb-remote HOST:PORT` - Connects to remote lldb-server
   - Appends your custom `attachCommands`
4. **Generates Source Mapping**: Auto-creates `settings set target.source-map` from `pathMappings`
5. **Spawns lldb-dap**: Launches the debug adapter with transformed configuration

## Troubleshooting

### Symbols Not Loading

**Symptom**: Stack traces show hex addresses instead of function names

**Solution**: Ensure the local `program` binary matches the remote binary exactly (same build-ID/UUID):

```bash
# Check build ID matches
lldb-objdump --arch-headers /path/to/local/binary
lldb-objdump --arch-headers /path/to/remote/binary
```

Add to `initCommands`:
```json
"initCommands": [
  "settings set symbols.enable-external-lookup true"
]
```

### Breakpoints Not Hitting

**Symptom**: Breakpoints show as unresolved or don't trigger

**Solutions**:

1. Verify source path mapping is correct:
```json
"initCommands": [
  "settings show target.source-map"
]
```

2. Set breakpoints in `attachCommands` with full paths:
```json
"attachCommands": [
  "breakpoint set --file /remote/path/to/file.c --line 42"
]
```

### Variable Editing Shows Error

**Known Issue**: Zed may show "missing field 'value'" error when editing variables in the UI, but the value is actually set correctly.

**Workaround**: Use the LLDB console to verify changes:
```
expr variable_name
```

### Connection Refused

**Symptom**: `error: failed to connect to 'HOST:PORT'`

**Solutions**:

1. Verify lldb-server is running on the target
2. Check firewall rules allow the port
3. Test connectivity: `nc -zv HOST PORT`

## Binary Name Configuration

The extension defaults to `lldb-dap-20`. If your system uses a different binary name:

1. Create a symlink:
```bash
sudo ln -s /usr/bin/lldb-dap /usr/bin/lldb-dap-20
```

Or modify `src/lib.rs:219` and rebuild the extension.

## Known Limitations

- Variable editing in Zed UI may show errors (workaround: use LLDB console)
- Breakpoints set via Zed UI may not resolve with source-map (use `attachCommands` instead)
- Extension runs in WASM environment - no access to environment variables at runtime

## Contributing

Contributions welcome! Please submit issues and pull requests to the [GitHub repository](https://github.com/xl4hub/zed-lldb-remote).

## License

Apache 2.0 - see [LICENSE](LICENSE) file for details

## Support

For issues and feature requests, please use the [GitHub issue tracker](https://github.com/xl4hub/zed-lldb-remote/issues).

## See Also

- [Zed Debugging Documentation](https://zed.dev/docs/debugging)
- [LLDB Documentation](https://lldb.llvm.org/)
- [Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/)
- [debuginfod Protocol](https://sourceware.org/elfutils/Debuginfod.html)
