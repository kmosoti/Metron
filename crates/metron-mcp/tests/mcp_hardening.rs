//! Wave 2 hardening tests for `metron-mcp`:
//! - Pre-sandbox JSON Schema validation (required fields, types, unknown fields)
//! - Bounded receipt ring buffer + durable `SqliteReceiptSink` into `metron-store-sqlite`
//! - Live cancellation propagation killing a runaway jailed child on `notifications/cancelled`

use metron_mcp::{
    McpBridgeError, MetronMcpClient, MetronMcpServer, SandboxedToolRegistry, SandboxedToolSpec,
    SqliteReceiptSink,
};
use metron_sandbox::{
    CgroupLimits, sha256_regular_file, systemd_user_available, verify_fd_hygiene,
};
use metron_store_sqlite::SqliteStore;
use rmcp::ServiceExt;
use serde_json::{Map, json};
use std::{
    fs,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, ExitCode},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

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
        println!("mcp_schema_validation_sqlite_sink_ring_and_cancellation: test");
        return ExitCode::SUCCESS;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    print!("test mcp_schema_validation_sqlite_sink_ring_and_cancellation ... ");
    rt.block_on(mcp_schema_validation_sqlite_sink_ring_and_cancellation());
    println!("ok");
    println!("test result: ok. 1 passed; 0 failed; 0 ignored");
    ExitCode::SUCCESS
}

async fn mcp_schema_validation_sqlite_sink_ring_and_cancellation() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp-mcp-tests");
    let root = base.join(format!("mcp-harden-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let scratch = root.join("scratch");
    fs::create_dir_all(&scratch).unwrap();

    let db_path = root.join("mcp.sqlite");
    let store = SqliteStore::create(&db_path).unwrap();
    let sink = Arc::new(SqliteReceiptSink::new(store, "task_mcp_harden").unwrap());

    let schema = match json!({
        "type": "object",
        "properties": {
            "count": { "type": "integer", "minimum": 1 },
            "label": { "type": "string", "minLength": 1 }
        },
        "required": ["count"],
        "additionalProperties": false
    }) {
        serde_json::Value::Object(map) => Arc::new(map),
        _ => unreachable!(),
    };

    let shell_path = fs::canonicalize("/bin/sh").unwrap();
    let shell_sha = sha256_regular_file(&shell_path).unwrap();
    let jail_bin = std::env::current_exe().unwrap();
    let use_systemd = systemd_user_available();

    let mut registry = SandboxedToolRegistry::new()
        .with_receipt_capacity(2)
        .with_receipt_sink(Arc::clone(&sink) as Arc<dyn metron_mcp::ReceiptSink>);
    registry
        .register(SandboxedToolSpec::LaneBNative {
            name: "quick_ok".to_owned(),
            description: "Quick pinned check".to_owned(),
            input_schema: Arc::clone(&schema),
            executable_path: shell_path.clone(),
            sha256: shell_sha.clone(),
            fixed_args: vec!["-c".to_owned(), "exit 0".to_owned()],
            workspace: None,
            writable_workspace: false,
            scratch: scratch.clone(),
            cgroup: CgroupLimits {
                scratch_reserve_bytes: 0,
                ..CgroupLimits::default()
            },
            trampoline_bin: Some(jail_bin.clone()),
            use_systemd,
        })
        .unwrap();
    registry
        .register(SandboxedToolSpec::LaneBNative {
            name: "slow_sleep".to_owned(),
            description: "Runaway tool for cancellation check".to_owned(),
            input_schema: schema,
            executable_path: shell_path,
            sha256: shell_sha,
            fixed_args: vec!["-c".to_owned(), "sleep 20".to_owned()],
            workspace: None,
            writable_workspace: false,
            scratch: scratch.clone(),
            cgroup: CgroupLimits {
                timeout_ms: 15_000,
                scratch_reserve_bytes: 0,
                ..CgroupLimits::default()
            },
            trampoline_bin: Some(jail_bin),
            use_systemd: false,
        })
        .unwrap();

    let registry = Arc::new(registry);
    let server = MetronMcpServer::new(Arc::clone(&registry));
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn({
        let server = server.clone();
        async move {
            let running = server.serve(server_io).await.unwrap();
            let _ = running.waiting().await;
        }
    });
    let client = MetronMcpClient::connect(client_io).await.unwrap();

    // 1. Schema validation rejects missing required field, wrong type, and unknown field BEFORE sandbox.
    assert!(
        client
            .call_sandboxed_tool("quick_ok", Map::new())
            .await
            .is_err()
    );
    let mut bad_type = Map::new();
    bad_type.insert("count".to_owned(), json!("not-an-int"));
    assert!(
        client
            .call_sandboxed_tool("quick_ok", bad_type)
            .await
            .is_err()
    );
    let mut unknown = Map::new();
    unknown.insert("count".to_owned(), json!(1));
    unknown.insert("extra".to_owned(), json!(true));
    assert!(
        client
            .call_sandboxed_tool("quick_ok", unknown)
            .await
            .is_err()
    );
    assert_eq!(
        server.receipts().len(),
        0,
        "invalid args must never reach sandbox"
    );

    // 2. Bounded ring buffer retains only capacity=2 while SqliteReceiptSink durably persists all 3.
    let mut good = Map::new();
    good.insert("count".to_owned(), json!(1));
    for _ in 0..3 {
        let out = client
            .call_sandboxed_tool("quick_ok", good.clone())
            .await
            .unwrap();
        assert!(!out.is_error);
    }
    assert_eq!(
        server.receipts().len(),
        2,
        "in-memory ring must stay bounded at capacity=2"
    );
    assert_eq!(
        sink.persisted_count().unwrap(),
        3,
        "SQLite sink must persist all 3 receipts"
    );

    // 3. Cancellation flag kills runaway jailed child promptly.
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_bg = Arc::clone(&cancel);
    let spec = registry.lookup("slow_sleep").unwrap();
    let reg_bg = Arc::clone(&registry);
    let started = Instant::now();
    let killer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        cancel_bg.store(true, Ordering::Release);
    });
    let res = tokio::task::spawn_blocking(move || {
        reg_bg.execute_spec_cancellable(&spec, &good, Some(&cancel))
    })
    .await
    .unwrap();
    killer.await.unwrap();
    assert_eq!(res.unwrap_err(), McpBridgeError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));

    client.close().await.unwrap();
    let _ = server_handle.await;
    let _ = fs::remove_dir_all(&root);
}
