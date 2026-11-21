//! Sandbox execution dispatcher for registered Lane A (`wasm_aot`) and Lane B (`native_jail`) MCP tools.

use crate::{
    receipt::{McpLaneKind, McpResourceReceipt, build_protocol_receipt, hex_sha256},
    registry::{McpBridgeError, SandboxedToolSpec},
};
use metron_sandbox::{
    ExecSource, LaneARequest, NamespacePolicy, NativeJailConfig, run_direct_jail_cancellable,
    run_lane_a_jail_cancellable, run_native_jail_cancellable, seal_bytes, userns_supported,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn run_sandboxed_spec(
    seq: u64,
    spec: &SandboxedToolSpec,
    args_json: &[u8],
    args_digest: String,
    cancel: Option<&AtomicBool>,
) -> Result<(Value, McpResourceReceipt), McpBridgeError> {
    match spec {
        SandboxedToolSpec::LaneAWasm {
            name,
            cwasm_bytes,
            meta,
            hmac_key,
            tunables,
            limits,
            scratch,
            trampoline_bin,
            use_systemd,
            ..
        } => {
            let memfd = seal_bytes("mcp-lane-a-cwasm", cwasm_bytes, None, true)
                .map_err(|e| McpBridgeError::Sandbox(e.to_string()))?;
            let mut lane_limits = limits.clone();
            lane_limits.cgroup.unit_name = format!("metron-mcp-a-{}-{seq}", std::process::id());
            let res = run_lane_a_jail_cancellable(
                &LaneARequest {
                    cwasm_memfd: &memfd,
                    meta,
                    key: hmac_key,
                    tunables,
                    limits: &lane_limits,
                    scratch,
                    stdin_bytes: args_json,
                    trampoline_bin: trampoline_bin.as_deref(),
                    use_systemd: *use_systemd,
                },
                cancel,
            )
            .map_err(|e| {
                if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
                    McpBridgeError::Cancelled
                } else {
                    McpBridgeError::Sandbox(e.to_string())
                }
            })?;
            let ok = res.exit_code == 0 && res.trapped.is_none();
            let diag = res.trapped.clone().unwrap_or_default();
            let proto = build_protocol_receipt(seq, name, &args_digest, &res.stdout, ok, &diag);
            let out_val = json!({
                "stdout": String::from_utf8_lossy(&res.stdout),
                "stderr": String::from_utf8_lossy(&res.stderr),
                "exit_code": res.exit_code,
                "trapped": res.trapped,
            });
            let rec = McpResourceReceipt {
                tool_name: name.clone(),
                lane: McpLaneKind::WasmAot,
                exit_code: res.exit_code,
                fuel_consumed: res.fuel_consumed,
                peak_memory_bytes: res.peak_memory_bytes,
                stdout_sha256: res.stdout_sha256,
                stderr_sha256: res.stderr_sha256,
                args_digest,
                trapped: res.trapped,
                protocol_receipt: proto,
            };
            Ok((out_val, rec))
        }
        SandboxedToolSpec::LaneBNative {
            name,
            executable_path,
            sha256,
            fixed_args,
            workspace,
            writable_workspace,
            scratch,
            cgroup,
            trampoline_bin,
            use_systemd,
            ..
        } => {
            let mut cg = cgroup.clone();
            cg.unit_name = format!("metron-mcp-b-{}-{seq}", std::process::id());
            let cfg = NativeJailConfig {
                cgroup: cg,
                primary: ExecSource::Path {
                    path: executable_path.clone(),
                    sha256: Some(sha256.clone()),
                    label: format!("mcp-tool-{name}"),
                },
                inherited: Vec::new(),
                args: fixed_args.clone(),
                workspace: workspace.clone(),
                writable_workspace: *writable_workspace,
                scratch: scratch.clone(),
                ro_inputs: Vec::new(),
                workdir: workspace.clone().unwrap_or_else(|| scratch.clone()),
                env: Vec::new(),
                trampoline_bin: trampoline_bin.clone(),
                namespace_policy: if userns_supported() {
                    NamespacePolicy::Require
                } else {
                    NamespacePolicy::AllowDegraded
                },
                allow_unix_sockets: false,
            };
            let stdin_fd = seal_bytes("mcp-lane-b-stdin", args_json, None, true)
                .map_err(|e| McpBridgeError::Sandbox(e.to_string()))?;
            let done = if *use_systemd {
                run_native_jail_cancellable(&cfg, Some(&stdin_fd), cancel)
            } else {
                run_direct_jail_cancellable(&cfg, Some(&stdin_fd), cancel)
            }
            .map_err(|e| {
                if cancel.is_some_and(|c| c.load(Ordering::Acquire)) {
                    McpBridgeError::Cancelled
                } else {
                    McpBridgeError::Sandbox(e.to_string())
                }
            })?;
            if cancel.is_some_and(|c| c.load(Ordering::Acquire)) && done.timed_out {
                return Err(McpBridgeError::Cancelled);
            }
            let exit_code = done.exit_code.unwrap_or(124);
            let ok = done.success && !done.timed_out;
            let trapped = done.timed_out.then(|| "timeout".to_owned());
            let diag = trapped.clone().unwrap_or_default();
            let proto = build_protocol_receipt(seq, name, &args_digest, &done.stdout, ok, &diag);
            let out_val = json!({
                "stdout": String::from_utf8_lossy(&done.stdout),
                "stderr": String::from_utf8_lossy(&done.stderr),
                "exit_code": exit_code,
                "timed_out": done.timed_out,
            });
            let rec = McpResourceReceipt {
                tool_name: name.clone(),
                lane: McpLaneKind::NativeJail,
                exit_code,
                fuel_consumed: 0,
                peak_memory_bytes: 0,
                stdout_sha256: hex_sha256(&done.stdout),
                stderr_sha256: hex_sha256(&done.stderr),
                args_digest,
                trapped,
                protocol_receipt: proto,
            };
            Ok((out_val, rec))
        }
    }
}
