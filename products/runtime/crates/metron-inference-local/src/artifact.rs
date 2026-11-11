//! Explicit installation and full-file verification for pinned model artifacts.

use sha2::{Digest as _, Sha256};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Artifact {
    Control,
    Challenger,
    Q6K,
    Q8Zero,
}

impl Artifact {
    pub const fn filename(self) -> &'static str {
        match self {
            Self::Control => "LFM3-1.7B-Q4_K_M.gguf",
            Self::Challenger => "LFM3-1.7B-QAD-Q4_0.gguf",
            Self::Q6K => "LFM3-1.7B-Q6_K.gguf",
            Self::Q8Zero => "LFM3-1.7B-Q8_0.gguf",
        }
    }

    pub const fn size(self) -> u64 {
        match self {
            Self::Control => 1_099_627_808,
            Self::Challenger => 1_049_615_680,
            Self::Q6K => 1_427_638_304,
            Self::Q8Zero => 1_846_238_496,
        }
    }

    pub const fn sha256(self) -> &'static str {
        match self {
            Self::Control => "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
            Self::Challenger => "43a7d09a0e9cba91b63eb7052488b9564569bddfd65754d2e426465e572a7452",
            Self::Q6K => "54e1bd947c77167823d45bf495f1c3e2d226ca7fb242f83cfa141ee3a6ab55c6",
            Self::Q8Zero => "8f6b24182258205e9e01e21f1f24a1f208c1e8115578825d202ce39976af20b6",
        }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn digest_reader(
    mut input: impl Read,
    mut output: impl Write,
    expected_size: u64,
) -> io::Result<String> {
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| invalid("artifact length overflow"))?;
        if total > expected_size {
            return Err(invalid("artifact exceeds pinned size"));
        }
        digest.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    if total != expected_size {
        return Err(invalid("artifact does not match pinned size"));
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Verify every byte against the immutable, approved artifact manifest.
pub fn verify_artifact(path: &Path, artifact: Artifact) -> io::Result<()> {
    let file = File::open(path)?;
    if file.metadata()?.len() != artifact.size() {
        return Err(invalid("artifact does not match pinned size"));
    }
    if digest_reader(file, io::sink(), artifact.size())? != artifact.sha256() {
        return Err(invalid("artifact does not match pinned SHA-256"));
    }
    Ok(())
}

/// Immutable auxiliary files associated with the pinned non-GGUF model revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactFile {
    pub filename: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub const AUXILIARY_FILES: &[ArtifactFile] = &[
    ArtifactFile {
        filename: "LICENSE",
        size: 10574,
        sha256: "4d28ca14dedc0b3d0fcc2b3339f0e79931faa33874f3d24f522183a8fc70068c",
    },
    ArtifactFile {
        filename: "README.md",
        size: 55086,
        sha256: "63ef157d747e4fd77376a6eca0d7177bbeb4a2db3c9f5baacfd6afaf5a338e55",
    },
    ArtifactFile {
        filename: "config.json",
        size: 1454,
        sha256: "8c6ae8b8f6d88c87cfb98cf4b5e34e943d0ee07e24215c4121b3159a46e1399d",
    },
    ArtifactFile {
        filename: "generation_config.json",
        size: 328,
        sha256: "0095ba7158e938b1e2d3082d412f00d8be85e42cbbd7608b63866f7a081cf270",
    },
    ArtifactFile {
        filename: "tokenizer.json",
        size: 17908635,
        sha256: "7e226e34bface23ccf6e495d157bf77186c1c30a7659880e1a8994db528c3701",
    },
    ArtifactFile {
        filename: "tokenizer_config.json",
        size: 1545,
        sha256: "bf028f813efc6137bc07ed045cafe71820df9a5e26a38e39954706ed1e30a457",
    },
    ArtifactFile {
        filename: "chat_template.jinja",
        size: 12129,
        sha256: "f45005c50cd2a97329722bf3a2dee9f16c04cdf825a589b280fb7e99e6827818",
    },
    ArtifactFile {
        filename: "special_tokens_map.json",
        size: 92,
        sha256: "7946f864cbfb62a1b041bda937244210a1a87f4a6cab3a48b791131b2cd57f88",
    },
];

pub fn verify_auxiliary(path: &Path, artifact: ArtifactFile) -> io::Result<()> {
    let file = File::open(path)?;
    if file.metadata()?.len() != artifact.size {
        return Err(invalid("artifact does not match pinned size"));
    }
    if digest_reader(file, io::sink(), artifact.size)? != artifact.sha256 {
        return Err(invalid("artifact does not match pinned SHA-256"));
    }
    Ok(())
}

/// Read and hash the exact pinned template before the SDK loads its path.
pub fn read_verified_template(directory: &Path) -> io::Result<String> {
    let entry = AUXILIARY_FILES
        .iter()
        .copied()
        .find(|file| file.filename == "chat_template.jinja")
        .expect("pinned template manifest entry");
    let file = File::open(directory.join(entry.filename))?;
    let mut bytes = Vec::with_capacity(entry.size as usize);
    let digest = digest_reader(file, &mut bytes, entry.size)?;
    if digest != entry.sha256 {
        return Err(invalid("template does not match pinned SHA-256"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("template is not UTF-8"))
}

/// Stream a regular executable into SHA-256 with a conservative 2 GiB bound.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
        return Err(invalid("invalid executable file"));
    }
    digest_reader(file, io::sink(), metadata.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamed_digest_rejects_short_and_long_input() {
        assert_eq!(
            digest_reader(&b"abc"[..], io::sink(), 3).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(digest_reader(&b"ab"[..], io::sink(), 3).is_err());
        assert!(digest_reader(&b"abcd"[..], io::sink(), 3).is_err());
    }
}
