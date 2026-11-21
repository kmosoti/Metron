//! End-to-end integration test proving an HTTP mock `ModelProvider` + `metron-mcp`
//! sandboxed `LaneAWasm` tool call runs through `metron-host` behind `AuthorizedEffect`
//! with durably persisted `ResourceReceipt`s and indexed SQLite `intents(task_id, status)`.

use super::http_backend::{HttpModelBackend, ModelBackend};
use super::*;
use crate::coding_tools::McpToolDispatcher;
use metron_mcp::{SandboxedToolRegistry, SandboxedToolSpec};
use metron_provider_http::{
    CircuitBreakerConfig, EndpointConfig, HttpModelProvider, ProviderKind, RetryPolicy,
};
use metron_sandbox::{
    AotEnvelopeMeta, EngineTunables, LaneALimits, PINNED_WASMTIME_VERSION, host_target_triple,
    systemd_user_available,
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag};
use serde_json::{Map, json};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use wasmtime::Engine;

const E2E_WASM_WAT: &str = r#"
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
        (br_if $work (i32.lt_s (local.get $i) (i32.const 150)))
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

fn compile_e2e_wasm(tunables: &EngineTunables, key: &[u8; 32]) -> (Vec<u8>, AotEnvelopeMeta) {
    let wasm = wat::parse_str(E2E_WASM_WAT).unwrap();
    let engine = Engine::new(&tunables.to_wasmtime_config().unwrap()).unwrap();
    let cwasm = engine.precompile_component(&wasm).unwrap();
    let target = host_target_triple().unwrap();
    let td = tunables.digest();
    let cb = *blake3::hash(&cwasm).as_bytes();
    let seal = AotSeal {
        wasmtime_version: PINNED_WASMTIME_VERSION,
        target_triple: target,
        engine_tunables_digest: td,
        cwasm_blake3: cb,
        hmac_tag: [0u8; 32],
    };
    let tag = compute_seal_tag(key, &seal).unwrap();
    let hex = |b: &[u8]| -> String {
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
            engine_tunables_digest_hex: hex(&td),
            cwasm_blake3_hex: hex(&cb),
            hmac_tag_hex: hex(&tag),
        },
    )
}

#[test]
fn http_mock_provider_and_mcp_lane_a_tool_persist_receipts_in_sqlite() {
    let (root, mut s, mut store) = tests::setup(
        "http_mcp_e2e",
        EngineMode::Agent,
        vec![CodingTool::Read, CodingTool::Finish],
        false,
    );
    // Allow non-zero micro-USD spend from the HTTP pricing table in principal & task budgets.
    let db_path = root.join("run/runtime.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let idx_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name='idx_intents_task_status'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1, "idx_intents_task_status index must exist");
    }

    // Spawn ephemeral local mock OpenAI HTTP server replying with a valid coding plan JSON.
    let reply_json = r#"{"action":"read_file","path":"src/lib.rs"}"#;
    let http_body = serde_json::json!({
        "choices": [{
            "message": { "content": reply_json },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 120,
            "completion_tokens": 24,
            "total_tokens": 144
        }
    })
    .to_string();

    let (tx_addr, rx_addr) = std::sync::mpsc::channel();
    let server_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tx_addr.send(format!("http://{addr}")).unwrap();
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{http_body}",
                http_body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
    });
    let base_url = rx_addr.recv().unwrap();

    // Create a fresh paid task with spend_micros_usd headroom for the HTTP pricing table.
    let task_id = TaskId::new("http_mcp_e2e_paid").unwrap();
    let mut paid_data = s.task.data().clone();
    paid_data.task_id = task_id.clone();
    paid_data.principal_id = PrincipalId::new("operator_http").unwrap();
    paid_data.budget.max_spend_micros_usd = SqlU64::new(100_000).unwrap();
    s.task = CodingTask::new(paid_data).unwrap();
    s.policy.principal_id = PrincipalId::new("operator_http").unwrap();
    store
        .apply(
            command(StoreOperation::CreateTask {
                task: Task::new_coding(s.task.clone()).unwrap(),
                genesis: s.genesis().unwrap(),
            })
            .unwrap(),
        )
        .unwrap();

    // Build real LaneAWasm MCP tool backed by SqliteReceiptSink pointing at runtime.sqlite.
    // Use a disk-backed scratch directory (avoiding tmpfs /tmp which Lane A/B intentionally reject).
    let scratch = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp-host-tests")
        .join(format!("http-mcp-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let key = [0x39u8; 32];
    let tunables = EngineTunables::default();
    let (cwasm, meta) = compile_e2e_wasm(&tunables, &key);
    let schema = match json!({
        "type": "object",
        "properties": { "input": { "type": "string" } },
        "required": ["input"],
        "additionalProperties": false
    }) {
        serde_json::Value::Object(map) => Arc::new(map),
        _ => unreachable!(),
    };
    let sink = McpToolDispatcher::sqlite_sink(&db_path, task_id.as_str()).unwrap();
    let mut registry = SandboxedToolRegistry::new()
        .with_receipt_capacity(16)
        .with_receipt_sink(sink);
    registry
        .register(SandboxedToolSpec::LaneAWasm {
            name: "wasm_check".to_owned(),
            description: "Sandboxed Lane A compute tool".to_owned(),
            input_schema: schema,
            cwasm_bytes: cwasm,
            meta,
            hmac_key: key,
            tunables,
            limits: LaneALimits {
                fuel_budget: 200_000,
                max_memory_bytes: 16 * 1024 * 1024,
                cgroup: metron_sandbox::CgroupLimits {
                    scratch_reserve_bytes: 0,
                    ..metron_sandbox::CgroupLimits::default()
                },
                ..LaneALimits::default()
            },
            scratch: scratch.clone(),
            trampoline_bin: None,
            use_systemd: systemd_user_available(),
        })
        .unwrap();
    let dispatcher = McpToolDispatcher::new(Arc::new(registry));

    // Configure Driver with ModelBackend::Http + McpToolDispatcher.
    let provider = HttpModelProvider::new(EndpointConfig {
        kind: ProviderKind::OpenAiCompatible,
        base_url,
        model: "gpt-4o".to_owned(),
        api_key: None,
        timeout_ms: 5_000,
        max_response_bytes: 32 * 1024,
        pricing_override: None,
        retry: RetryPolicy::default(),
        circuit_breaker: CircuitBreakerConfig::default(),
    })
    .unwrap();
    let driver = Driver {
        tools: CodingTools::new(
            &s.task,
            &s.policy,
            &s.catalog,
            &root.join("workspace"),
            &root.join("run"),
        )
        .unwrap(),
        model: ModelBackend::Http(Box::new(HttpModelBackend::new(
            provider,
            db_path.clone(),
            "You are a concise coding agent.",
            128,
        ))),
        mcp: Some(dispatcher.clone()),
        memory: None,
        store_path: db_path.clone(),
        run_dir: root.join("run"),
        policy: s.policy.clone(),
        context_version: s.context_version,
    };

    // Drive one AuthorizedEffect model step through metron-host (HTTP mock) + dispatch MCP tool
    // inside the same AuthorizedEffect boundary.
    let engine = crate::cognition(
        &s.genesis().unwrap(),
        &Task::new_coding(s.task.clone()).unwrap(),
    )
    .unwrap();
    struct InterceptDriver {
        inner: Driver,
        ran_mcp: bool,
    }
    impl EffectDriver for InterceptDriver {
        type Outcome = DriverOutcome;
        fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
            if !self.ran_mcp
                && let Some(mcp) = self.inner.mcp.clone()
            {
                let mut args = Map::new();
                args.insert("input".to_owned(), json!("verified"));
                let outcome = mcp
                    .dispatch_authorized(&effect, "wasm_check", args)
                    .expect("MCP LaneAWasm tool must execute behind AuthorizedEffect");
                assert!(!outcome.is_error);
                assert!(outcome.receipt.fuel_consumed > 0);
                self.ran_mcp = true;
            }
            self.inner.execute(effect)
        }
    }
    let mut intercept = InterceptDriver {
        inner: driver,
        ran_mcp: false,
    };
    let mut verifier = Verifier {
        spec: s.acceptance.clone(),
        catalog: s.catalog.clone(),
        run_dir: root.join("run"),
    };
    let _ = orchestrator::drive_coding_with_driver(
        &mut store,
        &task_id,
        engine.as_ref(),
        &mut NoObserver,
        &mut intercept,
        &mut verifier,
    );
    let saw_http_and_mcp = intercept.ran_mcp;
    server_thread.join().unwrap();
    assert!(saw_http_and_mcp, "MCP LaneAWasm tool must have executed");

    // Verify BOTH the HTTP ModelProvider ResourceReceipt AND the MCP LaneAWasm ResourceReceipt
    // were durably persisted into metron-store-sqlite.
    let receipts = store
        .list_external_receipts(Some(task_id.as_str()))
        .unwrap();
    assert!(
        receipts
            .iter()
            .any(|r| r.kind == "http_model" && r.tool_or_model == "gpt-4o"),
        "missing persisted http_model ResourceReceipt: {receipts:?}"
    );
    assert!(
        receipts
            .iter()
            .any(|r| r.kind == "mcp_lane_a_wasm" && r.tool_or_model == "wasm_check"),
        "missing persisted mcp_lane_a_wasm ResourceReceipt: {receipts:?}"
    );
    drop(intercept);
    drop(store);
    let _ = fs::remove_dir_all(&scratch);
    fs::remove_dir_all(root).unwrap();
}
