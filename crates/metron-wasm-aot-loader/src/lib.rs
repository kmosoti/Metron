//! Audited `< 80`-line AOT component loader: sole workspace `unsafe_code` boundary.
use hmac::{Hmac, Mac};
use rustix::fs::{SealFlags, fcntl_get_seals};
use sha2::Sha256;
use std::io::{Read, Seek, SeekFrom, copy};
use std::{fs::File, os::fd::AsRawFd};
use wasmtime::{Engine, component::Component};

/// Cryptographic seal metadata bound to a compiled `.cwasm` artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AotSeal<'a> {
    /// Pinned Wasmtime version string.
    pub wasmtime_version: &'a str,
    /// Target triple the `.cwasm` artifact was compiled for.
    pub target_triple: &'a str,
    /// BLAKE3 digest of deterministic engine runtime tunables.
    pub engine_tunables_digest: [u8; 32],
    /// BLAKE3 digest of the `.cwasm` bytes.
    pub cwasm_blake3: [u8; 32],
    /// Domain-separated HMAC-SHA256 tag over the bound fields.
    pub hmac_tag: [u8; 32],
}

fn seal_mac(key: &[u8; 32], s: &AotSeal<'_>) -> Result<Hmac<Sha256>, &'static str> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).map_err(|_| "invalid key")?;
    m.update(b"metron-wasm-aot/v1\0");
    for f in [s.wasmtime_version.as_bytes(), s.target_triple.as_bytes()] {
        m.update(&(f.len() as u32).to_le_bytes());
        m.update(f);
    }
    m.update(&s.engine_tunables_digest);
    m.update(&s.cwasm_blake3);
    Ok(m)
}

/// Computes the domain-separated HMAC-SHA256 seal tag for an AOT artifact.
pub fn compute_seal_tag(key: &[u8; 32], seal: &AotSeal<'_>) -> Result<[u8; 32], &'static str> {
    Ok(seal_mac(key, seal)?.finalize().into_bytes().into())
}

/// Verifies `F_SEAL_WRITE` memfd seals, BLAKE3, host tuple, and HMAC before `Component::deserialize_file`.
#[allow(unsafe_code)]
pub fn deserialize_verified_memfd(
    engine: &Engine,
    memfd: &File,
    key: &[u8; 32],
    seal: &AotSeal<'_>,
    expected: (&str, &str, &[u8; 32], usize),
) -> Result<Component, &'static str> {
    let req = SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL;
    if !fcntl_get_seals(memfd).is_ok_and(|s| s.contains(req)) {
        return Err("memfd missing immutable seals");
    }
    let (ver, target, tunables, max_bytes) = expected;
    if seal.wasmtime_version != ver
        || seal.target_triple != target
        || !seal.target_triple.starts_with(std::env::consts::ARCH)
        || seal.engine_tunables_digest != *tunables
    {
        return Err("cwasm host/tunables metadata mismatch");
    }
    let mut f = memfd.try_clone().map_err(|_| "memfd clone failed")?;
    f.seek(SeekFrom::Start(0))
        .map_err(|_| "memfd seek failed")?;
    let mut h = blake3::Hasher::new();
    let cap = max_bytes.saturating_add(1) as u64;
    if copy(&mut f.take(cap), &mut h).is_err_and(|_| true) || h.count() > max_bytes as u64 {
        return Err("cwasm unreadable or exceeds limit");
    }
    if *h.finalize().as_bytes() != seal.cwasm_blake3 {
        return Err("cwasm blake3 mismatch");
    }
    seal_mac(key, seal)?
        .verify_slice(&seal.hmac_tag)
        .map_err(|_| "cwasm hmac seal invalid")?;
    let path = format!("/proc/self/fd/{}", memfd.as_raw_fd());
    // SAFETY: `memfd` is `F_SEAL_WRITE`-immutable and passed BLAKE3, host tuple, and HMAC checks.
    unsafe { Component::deserialize_file(engine, path) }.map_err(|_| "component deserialize failed")
}
