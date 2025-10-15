use serde_json::Value;
use zed::{
    DebugAdapterBinary, Extension, Result, StartDebuggingRequestArguments,
    StartDebuggingRequestArgumentsRequest, Worktree,
};
use zed_extension_api as zed;

struct Ext {
    last_config_json: Option<String>,
    last_request_kind: Option<StartDebuggingRequestArgumentsRequest>,
}

/// Infer home directory from a path like /home/john/...
fn infer_home_from_path(path: &str) -> String {
    if let Some(start) = path.find("/home/") {
        if let Some(end) = path[start + 6..].find('/') {
            return format!("/home/{}", &path[start + 6..start + 6 + end]);
        }
    }
    std::env::var("HOME").unwrap_or_default()
}

/// Expand common variables in paths: ${HOME}, ${USER}
fn expand_variables(path: &str, home: &str) -> String {
    let mut result = path.to_string();

    if !home.is_empty() {
        result = result.replace("${HOME}", home);
        result = result.replace("$HOME", home);
    }

    // Extract username from home path like /home/john
    if let Some(user) = home.strip_prefix("/home/") {
        result = result.replace("${USER}", user);
        result = result.replace("$USER", user);
    }

    result
}

impl Extension for Ext {
    fn new() -> Self {
        Self {
            last_config_json: None,
            last_request_kind: None,
        }
    }

    // Capture the user's .zed/debug.json entry and decide attach/launch.
    fn dap_request_kind(
        &mut self,
        _adapter_name: String,
        config: Value,
    ) -> Result<StartDebuggingRequestArgumentsRequest> {
        // Save exact JSON to reuse later
        self.last_config_json = Some(config.to_string());

        // Decide request kind
        let req = match config
            .get("request")
            .and_then(|v| v.as_str())
            .unwrap_or("attach")
        {
            "launch" => StartDebuggingRequestArgumentsRequest::Launch,
            _ => StartDebuggingRequestArgumentsRequest::Attach,
        };
        self.last_request_kind = Some(req.clone());
        Ok(req)
    }

    // Spawn lldb-dap and pass only what it needs.
    fn get_dap_binary(
        &mut self,
        _adapter_name: String,
        _config: zed::DebugTaskDefinition,
        _user_provided_debug_adapter_path: Option<String>,
        worktree: &Worktree,
    ) -> Result<DebugAdapterBinary> {
        // Parse the captured JSON
        let cfg_in: serde_json::Value = self
            .last_config_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::json!({}));

        // Get home directory from worktree path
        let worktree_root = worktree.root_path();
        let home = infer_home_from_path(&worktree_root);

        // Always attach (that’s our scenario); compute the request enum
        let request = self
            .last_request_kind
            .clone()
            .unwrap_or(StartDebuggingRequestArgumentsRequest::Attach);

        // Extract tcp://HOST:PORT
        let tcp_addr = cfg_in
            .get("target")
            .and_then(|v| v.as_str())
            .and_then(|s| s.strip_prefix("tcp://"))
            .ok_or_else(|| "missing or invalid `target` (expected tcp://HOST:PORT)".to_string())?
            .to_string();

        // // Build the minimal lldb-dap configuration
        // // NOTE: we intentionally do NOT include program/pid/pathMappings here
        // let mut cfg_out = serde_json::json!({
        //     "request": "attach",
        //     "attachCommands": [ format!("gdb-remote {}", tcp_addr) ]
        // });
        // Build attach commands
        let mut attach_cmds = Vec::new();

        // If program is provided, create target BEFORE gdb-remote
        if let Some(program) = cfg_in.get("program").and_then(|v| v.as_str()) {
            let program = expand_variables(program, &home);
            attach_cmds.push(format!("target create {}", program));
        }

        // Then connect via gdb-remote
        attach_cmds.push(format!("gdb-remote {}", tcp_addr));

        // Then append user's attachCommands
        if let Some(post) = cfg_in.get("attachCommands").and_then(|v| v.as_array()) {
            for c in post {
                if let Some(s) = c.as_str() {
                    attach_cmds.push(s.to_string());
                }
            }
        }

        // Build outgoing configuration
        let mut cfg_out = serde_json::json!({
            "request": "attach",
            "attachCommands": attach_cmds
        });

        // Preserve stopOnEntry if present
        if let Some(soe) = cfg_in.get("stopOnEntry") {
            cfg_out
                .as_object_mut()
                .unwrap()
                .insert("stopOnEntry".into(), soe.clone());
        }

        // DO NOT forward program - we handle it in attachCommands instead
        // This prevents lldb-dap from loading symbols before gdb-remote connects

        // Forward pathMappings if present, with variable expansion
        if let Some(mappings) = cfg_in.get("pathMappings").and_then(|v| v.as_array()) {
            let expanded_mappings: Vec<serde_json::Value> = mappings
                .iter()
                .map(|mapping| {
                    let mut new_mapping = mapping.clone();
                    if let Some(obj) = new_mapping.as_object_mut() {
                        if let Some(local) = obj.get("localRoot").and_then(|v| v.as_str()) {
                            obj.insert("localRoot".into(), serde_json::json!(expand_variables(local, &home)));
                        }
                        if let Some(remote) = obj.get("remoteRoot").and_then(|v| v.as_str()) {
                            obj.insert("remoteRoot".into(), serde_json::json!(expand_variables(remote, &home)));
                        }
                    }
                    new_mapping
                })
                .collect();

            cfg_out
                .as_object_mut()
                .unwrap()
                .insert("pathMappings".into(), serde_json::json!(expanded_mappings));
        }

        // Forward env from debug.json (e.g., DEBUGINFOD_URLS) to the adapter process
        let mut envs: Vec<(String, String)> = Vec::new();
        if let Some(obj) = cfg_in.get("env").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    envs.push((k.clone(), s.to_string()));
                } else {
                    envs.push((k.clone(), v.to_string()));
                }
            }
        }

        // Build initCommands: start with user's, then add auto-generated source-map from pathMappings
        let mut init_cmds = Vec::new();

        // First, add user's initCommands if provided
        if let Some(inits) = cfg_in.get("initCommands").and_then(|v| v.as_array()) {
            for c in inits {
                if let Some(s) = c.as_str() {
                    init_cmds.push(s.to_string());
                }
            }
        }

        // Then auto-generate source-map settings from pathMappings
        if let Some(mappings) = cfg_in.get("pathMappings").and_then(|v| v.as_array()) {
            for mapping in mappings {
                if let (Some(remote), Some(local)) = (
                    mapping.get("remoteRoot").and_then(|v| v.as_str()),
                    mapping.get("localRoot").and_then(|v| v.as_str()),
                ) {
                    // Expand common variables in paths
                    let remote = expand_variables(remote, &home);
                    let local = expand_variables(local, &home);
                    init_cmds.push(format!("settings set target.source-map {} {}", remote, local));
                }
            }
        }

        // Add initCommands to config if we have any
        if !init_cmds.is_empty() {
            if let Some(obj) = cfg_out.as_object_mut() {
                obj.insert("initCommands".into(), serde_json::json!(init_cmds));
            }
        }

        Ok(DebugAdapterBinary {
            command: Some("lldb-dap-20".to_string()), // or "lldb-dap" if you symlinked
            arguments: vec![],
            cwd: None,
            envs,
            request_args: StartDebuggingRequestArguments {
                configuration: cfg_out.to_string(),
                request,
            },
            connection: None,
        })
    }
}

zed::register_extension!(Ext);
