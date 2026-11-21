//! End-to-end `rmcp 3.11` client <-> server bridge tests routing tools through `metron-sandbox`.

use metron_mcp::{
    McpLaneKind, MetronMcpClient, MetronMcpServer, SandboxedToolRegistry, SandboxedToolSpec,
};
use metron_sandbox::{
    AotEnvelopeMeta, CgroupLimits, EngineTunables, LaneALimits, PINNED_WASMTIME_VERSION,
    host_target_triple, sha256_regular_file, systemd_user_available, verify_fd_hygiene,
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag};
use rmcp::ServiceExt;
use serde_json::{Map, json};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    sync::Arc,
};
use wasmtime::Engine;

const COMPUTE_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 4)
    (func (export "run") (result i32)
      (local $i i32)
      (drop (memory.grow (i32.const 1)))
      (loop $work
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br_if $work (i32.lt_s (local.get $i) (i32.const 250)))
      )
      i32.const 0
    )
  )
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run))
)
"#;

fn compile_wat(tunables: &EngineTunables, key: &[u8; 32]) -> (Vec<u8>, AotEnvelopeMeta) {
    let wasm = wat::parse_str(COMPUTE_WAT).unwrap();
    let cfg = tunables.to_wasmtime_config().unwrap();
    let engine = Engine::new(&cfg).unwrap();
    let cwasm = engine.precompile_component(&wasm).unwrap();
    let target = host_target_triple().unwrap();
    let tunables_digest = tunables.digest();
    let cwasm_blake3 = *blake3::hash(&cwasm).as_bytes();
    let seal = AotSeal {
        wasmtime_version: PINNED_WASMTIME_VERSION,
        target_triple: target,
        engine_tunables_digest: tunables_digest,
        cwasm_blake3,
        hmac_tag: [0u8; 32],
    };
    let tag = compute_seal_tag(key, &seal).unwrap();
    let encode = |b: &[u8]| -> String {
        const H: &[u8; 16] = b"0123456789abcdef";
        b.iter()
            .flat_map(|&x| [H[(x >> 4) as usize] as char, H[(x & 0x0f) as usize] as char])
            .collect()
    };
    (
        cwasm,
        AotEnvelopeMeta {
            wasmtime_version: PINNED_WASMTIME_VERSION.to_owned(),
            target_triple: target.to_owned(),
            engine_tunables_digest_hex: encode(&tunables_digest),
            cwasm_blake3_hex: encode(&cwasm_blake3),
            hmac_tag_hex: encode(&tag),
        },
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(String::as_str),
        Some("--policy-file" | "--lane-a-policy")
    ) {
        return metron_sandbox::trampoline::run_from_args(&args);
    }
    if std::env::var_os("METRON_TEST_CLEAN_FD").is_none() && verify_fd_hygiene(&[]).is_err() {
        let exe = std::env::current_exe().expect("current_exe");
        let err = Command::new("bash")
            .args([
                "-c",
                "for p in /proc/$$/fd/*; do f=${p##*/}; case \"$f\" in *[!0-9]*|\"\") continue;; esac; [ \"$f\" -gt 2 ] && eval \"exec ${f}>&-\" 2>/dev/null || true; done; exec \"$@\"",
                "_",
            ])
            .arg(exe)
            .args(&args)
            .env("METRON_TEST_CLEAN_FD", "1")
            .exec();
        panic!("clean-fd re-exec failed: {err}");
    }
    if args.iter().any(|a| a == "--list") {
        println!("mcp_bridge_routes_lane_a_and_lane_b_through_sandbox_and_emits_receipts: test");
        return ExitCode::SUCCESS;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    print!("test mcp_bridge_routes_lane_a_and_lane_b_through_sandbox_and_emits_receipts ... ");
    rt.block_on(mcp_bridge_routes_lane_a_and_lane_b_through_sandbox_and_emits_receipts());
    println!("ok");
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
    ExitCode::SUCCESS
}

async fn mcp_bridge_routes_lane_a_and_lane_b_through_sandbox_and_emits_receipts() {
    let dir = TempDir::new("metron-mcp-bridge");
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();

    let jail_bin = std::env::current_exe().unwrap();
    let use_systemd = systemd_user_available();
    let key = [0x24u8; 32];
    let tunables = EngineTunables::default();
    let (cwasm, meta) = compile_wat(&tunables, &key);

    let schema = match json!({
        "type": "object",
        "properties": { "input": { "type": "string" } },
        "additionalProperties": false
    }) {
        serde_json::Value::Object(map) => Arc::new(map),
        _ => unreachable!(),
    };

    let mut registry = SandboxedToolRegistry::new();
    registry
        .register(SandboxedToolSpec::LaneAWasm {
            name: "wasm_compute".to_owned(),
            description: "Deterministic Lane A AOT component compute".to_owned(),
            input_schema: Arc::clone(&schema),
            cwasm_bytes: cwasm,
            meta,
            hmac_key: key,
            tunables,
            limits: LaneALimits {
                fuel_budget: 200_000,
                max_memory_bytes: 16 * 1024 * 1024,
                ..LaneALimits::default()
            },
            scratch: scratch.clone(),
            trampoline_bin: Some(jail_bin.clone()),
            use_systemd,
        })
        .unwrap();

    // Resolve the canonical non-symlink POSIX shell (`/usr/bin/dash` on Ubuntu,
    // `/usr/bin/bash` on Fedora) so `O_NOFOLLOW` + SHA-256 `memfd` sealing works
    // across distros where `/usr/bin/true` is a symlink to a multicall binary.
    let shell_path = fs::canonicalize("/bin/sh").unwrap();
    let shell_sha = sha256_regular_file(&shell_path).unwrap();
    registry
        .register(SandboxedToolSpec::LaneBNative {
            name: "pinned_true".to_owned(),
            description: "SHA-256 pinned Lane B check".to_owned(),
            input_schema: schema,
            executable_path: shell_path,
            sha256: shell_sha,
            fixed_args: vec!["-c".to_owned(), "exit 0".to_owned()],
            workspace: None,
            writable_workspace: false,
            scratch: scratch.clone(),
            cgroup: CgroupLimits::default(),
            trampoline_bin: Some(jail_bin),
            use_systemd,
        })
        .unwrap();

    let server = MetronMcpServer::new(Arc::new(registry));
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    let server_handle = tokio::spawn({
        let server = server.clone();
        async move {
            let running = server.serve(server_io).await.unwrap();
            let _ = running.waiting().await;
        }
    });

    let client = MetronMcpClient::connect(client_io).await.unwrap();
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2);

    let mut args = Map::new();
    args.insert("input".to_owned(), json!("deterministic"));

    let a1 = client
        .call_sandboxed_tool("wasm_compute", args.clone())
        .await
        .unwrap();
    let a2 = client
        .call_sandboxed_tool("wasm_compute", args.clone())
        .await
        .unwrap();
    assert!(!a1.is_error);
    assert_eq!(a1.receipt.lane, McpLaneKind::WasmAot);
    assert_eq!(a1.receipt.exit_code, 0);
    assert!(a1.receipt.fuel_consumed > 0);
    assert!(a1.receipt.peak_memory_bytes >= 128 * 1024);
    assert!(a1.receipt.protocol_receipt.is_some());
    assert_eq!(a1.receipt.fuel_consumed, a2.receipt.fuel_consumed);
    assert_eq!(a1.receipt.peak_memory_bytes, a2.receipt.peak_memory_bytes);

    let b1 = client
        .call_sandboxed_tool("pinned_true", args.clone())
        .await
        .unwrap();
    assert!(!b1.is_error);
    assert_eq!(b1.receipt.lane, McpLaneKind::NativeJail);
    assert_eq!(b1.receipt.exit_code, 0);
    assert!(b1.receipt.protocol_receipt.is_some());

    // Unregistered tools are rejected cleanly at the rmcp protocol boundary.
    assert!(
        client
            .call_sandboxed_tool("rm_rf_host", args)
            .await
            .is_err()
    );

    assert_eq!(server.receipts().len(), 3);
    client.close().await.unwrap();
    let _ = server_handle.await;
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let base = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        Self(base)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
