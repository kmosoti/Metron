//! Split Cranelift AOT compiler for Lane A (`wasm_aot`) artifacts.
//!
//! Compiled ONLY with `--features compiler` (or during unit/integration tests);
//! production runtime builds exclude Cranelift completely.

use crate::{
    error::SandboxError,
    wasm_aot::{
        AotEnvelopeMeta, EngineTunables, PINNED_WASMTIME_VERSION, encode_hex, host_target_triple,
    },
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag};
use wasmtime::Engine;

/// Compiles WebAssembly binary bytes (or WAT text) into a `.cwasm` image and signs
/// a domain-separated HMAC-SHA256 + BLAKE3 seal bound to `(version, triple, tunables, bytes)`.
pub fn compile_and_seal_component(
    wasm_or_wat: &[u8],
    tunables: &EngineTunables,
    key: &[u8; 32],
) -> Result<(Vec<u8>, AotEnvelopeMeta), SandboxError> {
    let component_bytes = normalize_component_bytes(wasm_or_wat)?;
    let cfg = tunables.to_wasmtime_config()?;
    let engine = Engine::new(&cfg)
        .map_err(|e| SandboxError::Denied(format!("compiler engine init failed: {e}")))?;
    let cwasm = engine
        .precompile_component(&component_bytes)
        .map_err(|e| SandboxError::Denied(format!("cwasm precompile failed: {e}")))?;

    let target = host_target_triple()?;
    let tunables_digest = tunables.digest();
    let cwasm_blake3 = *blake3::hash(&cwasm).as_bytes();
    let seal = AotSeal {
        wasmtime_version: PINNED_WASMTIME_VERSION,
        target_triple: target,
        engine_tunables_digest: tunables_digest,
        cwasm_blake3,
        hmac_tag: [0u8; 32],
    };
    let tag = compute_seal_tag(key, &seal).map_err(|e| SandboxError::Denied(e.into()))?;
    Ok((
        cwasm,
        AotEnvelopeMeta {
            wasmtime_version: PINNED_WASMTIME_VERSION.to_owned(),
            target_triple: target.to_owned(),
            engine_tunables_digest_hex: encode_hex(&tunables_digest),
            cwasm_blake3_hex: encode_hex(&cwasm_blake3),
            hmac_tag_hex: encode_hex(&tag),
        },
    ))
}

fn normalize_component_bytes(raw: &[u8]) -> Result<Vec<u8>, SandboxError> {
    if raw.starts_with(b"\0asm") {
        return Ok(raw.to_vec());
    }
    let parsed = wat::parse_bytes(raw)
        .map_err(|e| SandboxError::Denied(format!("WAT parse failed: {e}")))?;
    Ok(parsed.into_owned())
}
