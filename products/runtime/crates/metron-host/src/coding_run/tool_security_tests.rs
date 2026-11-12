//! Real admitted tools and OS-boundary negative controls; opt in on a qualified host.
use super::tests::{assert_audits, driver, setup_at};
use super::*;
use metron_protocol::coding::RustProgram;

fn fixture(name: &str) -> (PathBuf, Settings, SqliteStore) {
    let catalog = std::env::var("METRON_CODING_PROBE_CATALOG").expect("pinned probe catalog");
    let (root, mut s, old) = setup_at(
        Path::new(&catalog).parent().unwrap(),
        name,
        EngineMode::Harness,
        vec![
            CodingTool::List,
            CodingTool::Read,
            CodingTool::Run,
            CodingTool::Finish,
        ],
        false,
    );
    drop(old);
    s.catalog = read(Path::new(&catalog)).unwrap();
    let mut data = s.task.data().clone();
    data.tool_catalog_digest = s.catalog.digest().unwrap();
    s.task = CodingTask::new(data).unwrap();
    s.policy.allowed_programs = vec![RustProgram::Cargo, RustProgram::Rustc];
    s.policy.command_timeout_ms = 30_000;
    s.policy.command_memory_bytes = SqlU64::new(1024 * 1024 * 1024).unwrap();
    s.policy.max_processes = 64;
    let mut store = SqliteStore::create(root.join("run/security-runtime.sqlite")).unwrap();
    store
        .apply(
            command(StoreOperation::CreateTask {
                task: Task::new_coding(s.task.clone()).unwrap(),
                genesis: s.genesis().unwrap(),
            })
            .unwrap(),
        )
        .unwrap();
    (root, s, store)
}
fn write_project(root: &Path, build: &str) {
    let workspace = root.join("workspace");
    fs::create_dir(workspace.join("src")).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname=\"tool_probe\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )
    .unwrap();
    fs::write(
        workspace.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"tool_probe\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(workspace.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(workspace.join("build.rs"), build).unwrap();
}
#[test]
#[ignore = "requires pinned local tools, user systemd, bubblewrap and disk scratch"]
fn admitted_list_read_and_argv_cannot_escape_build_script() {
    if std::env::var_os("METRON_SECURITY_TEST_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "coding_run::tool_security_tests::admitted_list_read_and_argv_cannot_escape_build_script", "--ignored", "--nocapture"])
            .env("METRON_SECURITY_TEST_CHILD", "1").env("METRON_SECURITY_SENTINEL", "private-test-credential")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    assert_eq!(
        std::env::var("METRON_SECURITY_SENTINEL").unwrap(),
        "private-test-credential"
    );
    let (root, s, mut store) = fixture("security-argv");
    struct WorkspaceCleanup(PathBuf);
    impl Drop for WorkspaceCleanup {
        fn drop(&mut self) {
            let active = fs::read_dir(&self.0).ok().is_some_and(|entries| {
                entries.flatten().any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(".metron-active-")
                })
            });
            if !active {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    let cleanup = WorkspaceCleanup(root.clone());
    write_project(
        &root,
        r#"fn main() {
        assert!(!std::path::Path::new("/home").exists());
        assert_eq!(std::env::var("HOME").unwrap(), "/scratch");
        assert!(std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none());
        assert!(std::env::var_os("XDG_RUNTIME_DIR").is_none());
        assert!(std::env::var_os("METRON_SECURITY_SENTINEL").is_none());
        assert!(std::fs::write("/home/metron-escape", b"bad").is_err());
        let addr = "1.1.1.1:443".parse().unwrap();
        assert!(std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_err());
        println!("cargo:warning=security-probe-passed");
    }"#,
    );
    let script = [
        r#"{"tool":"plan","steps":["Inspect and run the confined project"]}"#,
        r#"{"tool":"list","directory":null}"#,
        r#"{"tool":"read","path":"src/main.rs","first_line":1,"line_count":10}"#,
        r#"{"tool":"run","program":"cargo","args":["check","--locked","-vv"],"directory":null}"#,
        r#"{"tool":"run","program":"rustc","args":["--edition=2024","src/main.rs","-o","/scratch/direct-probe"],"directory":null}"#,
        r#"{"tool":"finish","summary":"request verification"}"#,
    ];
    let mut worker = driver(&root, &s, &script);
    worker.store_path = root.join("run/security-runtime.sqlite");
    let task = Task::new_coding(s.task.clone()).unwrap();
    let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
    assert_eq!(
        orchestrator::drive_with_driver(
            &mut store,
            &s.task.data().task_id,
            engine.as_ref(),
            &root.join("run"),
            &mut NoObserver,
            &mut worker
        )
        .unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    let mut snapshot = store.audit().unwrap();
    let mut outputs = Vec::new();
    let mut cursor = None;
    loop {
        let page = snapshot.intents(&s.task.data().task_id, cursor).unwrap();
        for intent in page.records {
            for e in snapshot.evidence(&intent.intent_id, None).unwrap().records {
                if let ReceiptOutput::Coding { observation } = &e.receipt.data().output
                    && let CodingObservation::Tool { output } = observation.as_ref()
                {
                    assert_eq!(
                        output.status,
                        ToolStatus::Succeeded,
                        "{:?}: {}",
                        output.tool,
                        output.body.as_str()
                    );
                    assert!(output.process_tree_closed);
                    outputs.push(output.clone());
                }
            }
        }
        if let Some(next) = page.next {
            cursor = Some(next);
        } else {
            break;
        }
    }
    assert_eq!(
        outputs.iter().map(|x| x.tool).collect::<Vec<_>>(),
        vec![
            CodingTool::List,
            CodingTool::Read,
            CodingTool::Run,
            CodingTool::Run
        ]
    );
    assert!(outputs[0].body.as_str().contains("Cargo.toml"));
    assert!(outputs[1].body.as_str().contains("fn main"));
    assert!(outputs[2].body.as_str().contains("security-probe-passed"));
    assert_eq!(outputs[3].exit_code, Some(0));
    drop(snapshot);
    assert_audits(&mut store, &s);
    drop(worker);
    drop(store);
    drop(cleanup);
}

#[test]
#[ignore = "requires pinned local tools, user systemd, bubblewrap and disk scratch"]
fn live_cargo_host_kill_holds_workspace_and_reservation() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let signal = if let Ok(signal) = std::env::var("METRON_TOOL_KILL_CHILD_SIGNAL") {
        let (root, s, mut store) = fixture("security-host-kill");
        write_project(
            &root,
            r#"fn main() {
            std::fs::write("/workspace/live-start", b"started").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(25));
        }"#,
        );
        write_new(
            &root.join("settings.json"),
            &canonical_bytes(&s, MAX_INTERNAL).unwrap(),
        )
        .unwrap();
        fs::write(&signal, root.to_str().unwrap()).unwrap();
        let script = [
            r#"{"tool":"plan","steps":["Run the test build"]}"#,
            r#"{"tool":"run","program":"cargo","args":["check","--locked"],"directory":null}"#,
        ];
        let mut worker = driver(&root, &s, &script);
        worker.store_path = root.join("run/security-runtime.sqlite");
        let task = Task::new_coding(s.task.clone()).unwrap();
        let engine = crate::cognition(&s.genesis().unwrap(), &task).unwrap();
        let _ = orchestrator::drive_with_driver(
            &mut store,
            &s.task.data().task_id,
            engine.as_ref(),
            &root.join("run"),
            &mut NoObserver,
            &mut worker,
        );
        panic!("host was not killed during the live build");
    } else {
        Path::new(&std::env::var("METRON_CODING_PROBE_CATALOG").expect("catalog"))
            .parent()
            .unwrap()
            .join(format!("host-kill-signal-{}", std::process::id()))
    };
    struct Cleanup {
        root: Option<PathBuf>,
        signal: PathBuf,
        stopped: bool,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if self.root.is_none() {
                self.root = fs::read_to_string(&self.signal)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from);
            }
            if let Some(root) = &self.root {
                let mut safe = self.stopped;
                if let Ok(entries) = fs::read_dir(root) {
                    let markers = entries
                        .flatten()
                        .filter(|e| {
                            e.file_name()
                                .to_string_lossy()
                                .starts_with(".metron-active-")
                        })
                        .collect::<Vec<_>>();
                    if markers.is_empty() {
                        safe = true;
                    }
                    for entry in markers {
                        if !safe && let Ok(unit) = fs::read_to_string(entry.path()) {
                            safe = Command::new("/usr/bin/systemctl")
                                .args(["--user", "stop", unit.trim()])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status()
                                .is_ok_and(|s| s.success());
                        }
                    }
                }
                if safe {
                    let _ = fs::remove_dir_all(root);
                }
            }
            let _ = fs::remove_file(&self.signal);
        }
    }
    let mut cleanup = Cleanup {
        root: None,
        signal: signal.clone(),
        stopped: false,
    };
    struct KillChild(std::process::Child);
    impl Drop for KillChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "coding_run::tool_security_tests::live_cargo_host_kill_holds_workspace_and_reservation",
            "--ignored",
            "--nocapture",
        ])
        .env("METRON_TOOL_KILL_CHILD_SIGNAL", &signal)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = KillChild(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !signal.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "probe child exited before setup"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let root = PathBuf::from(fs::read_to_string(&signal).unwrap());
    cleanup.root = Some(root.clone());
    while !root.join("workspace/live-start").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "probe child exited before live process"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let marker = fs::read_dir(&root)
        .unwrap()
        .map(Result::unwrap)
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".metron-active-")
        })
        .unwrap()
        .path();
    let unit = fs::read_to_string(&marker).unwrap();
    let info = Command::new("/usr/bin/systemctl")
        .args([
            "--user",
            "show",
            unit.trim(),
            "-p",
            "ControlGroup",
            "--value",
        ])
        .output()
        .unwrap();
    assert!(info.status.success());
    let group = String::from_utf8(info.stdout).unwrap();
    let group = group.trim();
    assert!(group.starts_with('/'));
    let events = Path::new("/sys/fs/cgroup")
        .join(group.trim_start_matches('/'))
        .join("cgroup.events");
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(
        Command::new("/usr/bin/systemctl")
            .args(["--user", "stop", unit.trim()])
            .status()
            .unwrap()
            .success()
    );
    match fs::read_to_string(events) {
        Ok(value) => assert!(value.lines().any(|line| line == "populated 0")),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
    cleanup.stopped = true;
    assert!(marker.exists(), "unresolved dispatch marker was lost");
    let settings: Settings = read(&root.join("settings.json")).unwrap();
    let error = CodingTools::new(
        &settings.task,
        &settings.policy,
        &settings.catalog,
        &root.join("workspace"),
        &root.join("run"),
    )
    .err()
    .expect("workspace reuse must be denied");
    assert!(error.contains("unresolved process dispatch"), "{error}");
    let mut store = SqliteStore::open_existing(root.join("run/security-runtime.sqlite")).unwrap();
    let mut snapshot = store.audit().unwrap();
    let projection = snapshot.projection(&settings.task.data().task_id).unwrap();
    assert_eq!(projection.task_account.reserved.tool_calls.get(), 1);
    let intents = snapshot
        .intents(&settings.task.data().task_id, None)
        .unwrap();
    let run = intents
        .records
        .iter()
        .find(|i| i.reservation.is_some() && i.selected_evidence.is_none())
        .expect("live process reservation");
    assert!(run.selected_evidence.is_none());
    assert!(
        snapshot
            .evidence(&run.intent_id, None)
            .unwrap()
            .records
            .is_empty()
    );
    let count = intents.records.len();
    drop(snapshot);
    drop(store);
    let mut store = SqliteStore::open_existing(root.join("run/security-runtime.sqlite")).unwrap();
    let error = CodingTools::new(
        &settings.task,
        &settings.policy,
        &settings.catalog,
        &root.join("workspace"),
        &root.join("run"),
    )
    .err()
    .expect("workspace reuse must be denied");
    assert!(error.contains("unresolved process dispatch"), "{error}");
    let advance = command(StoreOperation::AdvanceTick {
        expected_tick: SqlU64::new(0).unwrap(),
        new_tick: SqlU64::new(30).unwrap(),
    })
    .unwrap();
    store.apply(advance).unwrap();
    struct CountingDriver(usize);
    impl EffectDriver for CountingDriver {
        type Outcome = DriverOutcome;
        fn execute(&mut self, _: AuthorizedEffect) -> DriverOutcome {
            self.0 += 1;
            DriverOutcome::Uncertain
        }
    }
    let task = Task::new_coding(settings.task.clone()).unwrap();
    let engine = crate::cognition(&settings.genesis().unwrap(), &task).unwrap();
    let mut counter = CountingDriver(0);
    assert_eq!(
        orchestrator::drive_with_driver(
            &mut store,
            &settings.task.data().task_id,
            engine.as_ref(),
            &root.join("run"),
            &mut NoObserver,
            &mut counter
        )
        .unwrap(),
        orchestrator::DriveStatus::Blocked
    );
    assert_eq!(counter.0, 0, "restart redispatched the uncertain process");
    let mut snapshot = store.audit().unwrap();
    let after = snapshot
        .intents(&settings.task.data().task_id, None)
        .unwrap();
    assert_eq!(after.records.len(), count);
    assert!(after.records.iter().any(|i| i.status
        == metron_protocol::store::DispatchStatus::Uncertain
        && i.reservation.is_some()
        && i.selected_evidence.is_none()));
    drop(snapshot);
    drop(store);
    drop(cleanup);
}
