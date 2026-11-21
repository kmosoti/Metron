//! Split AOT compiler binary (`metron-wasm-compiler`) gated behind `--features compiler`.
//!
//! Jails itself under Lane B (`Landlock` read-only input + write-only output dir +
//! zero-network/zero-exec `seccomp-bpf`) BEFORE reading untrusted Wasm/WAT or invoking Cranelift.

use metron_sandbox::{
    EngineTunables, LandlockPolicy, SeccompPolicy, apply_landlock, install_seccomp,
    wasm_aot::decode_hex_32, wasm_compiler::compile_and_seal_component,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("metron-wasm-compiler: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn canonical_parent(path: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::canonicalize(parent).map_err(|e| format!("output dir {}: {e}", parent.display()))
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(
            "usage: metron-wasm-compiler <input.wasm|wat> <out.cwasm> <out.seal.json> (key via METRON_AOT_HMAC_KEY)".into(),
        );
    }
    let key_hex = std::env::var("METRON_AOT_HMAC_KEY")
        .map_err(|_| "METRON_AOT_HMAC_KEY env var required".to_owned())?;
    let key = decode_hex_32(key_hex.trim()).map_err(|e| e.to_string())?;

    let input_path =
        fs::canonicalize(PathBuf::from(&args[0])).map_err(|e| format!("input path: {e}"))?;
    let out_cwasm = PathBuf::from(&args[1]);
    let out_meta = PathBuf::from(&args[2]);
    let cwasm_dir = canonical_parent(&out_cwasm)?;
    let meta_dir = canonical_parent(&out_meta)?;

    // Confine Cranelift under Landlock (read-only input + write-only output dirs) and
    // zero-network/zero-exec seccomp BEFORE reading input bytes or compiling.
    let mut wo_paths = vec![cwasm_dir];
    if !wo_paths.contains(&meta_dir) {
        wo_paths.push(meta_dir);
    }
    let mut landlock = LandlockPolicy::lane_b(vec![input_path.clone()], Vec::new());
    landlock.wo_paths = wo_paths;
    rustix::thread::set_no_new_privs(true).map_err(|e| format!("no_new_privs: {e}"))?;
    apply_landlock(&landlock).map_err(|e| e.to_string())?;
    install_seccomp(SeccompPolicy {
        deny_prot_exec: false,
        allow_unix_sockets: false,
        deny_exec: true,
    })
    .map_err(|e| e.to_string())?;

    let input = fs::read(&input_path).map_err(|e| format!("read failed: {e}"))?;
    let (cwasm, meta) = compile_and_seal_component(&input, &EngineTunables::default(), &key)
        .map_err(|e| e.to_string())?;
    fs::write(&out_cwasm, &cwasm).map_err(|e| format!("write cwasm failed: {e}"))?;
    let json = serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?;
    fs::write(&out_meta, &json).map_err(|e| format!("write meta failed: {e}"))?;
    Ok(())
}
