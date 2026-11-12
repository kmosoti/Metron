//! Bounded, deterministic compatibility tokenizer derived from pinned LFM3 inputs.
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

const MAX_METADATA: u64 = 64 * 1024 * 1024;
const MAX_STRING: u64 = 1024 * 1024;
const MAX_ITEMS: u64 = 1_000_000;
const MAX_JSON: u64 = 64 * 1024 * 1024;
const NOFOLLOW_NONBLOCK: i32 = 0x20000 | 0x800;
const REGEX: &str = r"(?:'[sS]|'[tT]|'[rR][eE]|'[vV][eE]|'[mM]|'[lL][lL]|'[dD])|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+";

pub struct PreparedTokenizer {
    pub path: PathBuf,
    pub sha256: String,
}

struct Reader<R> {
    inner: R,
    used: u64,
}
impl<R: Read> Reader<R> {
    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, String> {
        self.used = self
            .used
            .checked_add(n as u64)
            .filter(|n| *n <= MAX_METADATA)
            .ok_or("metadata byte limit")?;
        let mut v = vec![0; n];
        self.inner
            .read_exact(&mut v)
            .map_err(|_| "truncated GGUF metadata")?;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u64()?;
        if n > MAX_STRING {
            return Err("GGUF string limit".into());
        }
        String::from_utf8(self.bytes(n as usize)?).map_err(|_| "non-UTF-8 GGUF string".into())
    }
    fn value(&mut self, kind: u32, wanted: bool) -> Result<Meta, String> {
        match kind {
            8 => {
                let s = self.string()?;
                Ok(if wanted { Meta::Text(s) } else { Meta::Other })
            }
            9 => {
                let ty = self.u32()?;
                let n = self.u64()?;
                if n > MAX_ITEMS || ty == 9 {
                    return Err("GGUF array limit or nested array".into());
                }
                if wanted && ty == 8 {
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        v.push(self.string()?);
                    }
                    Ok(Meta::Strings(v))
                } else if wanted && ty == 5 {
                    let mut v = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        v.push(self.u32()? as i32);
                    }
                    Ok(Meta::Ints(v))
                } else {
                    for _ in 0..n {
                        self.value(ty, false)?;
                    }
                    Ok(Meta::Other)
                }
            }
            0 | 1 => {
                self.bytes(1)?;
                Ok(Meta::Other)
            }
            7 => {
                if self.bytes(1)?[0] > 1 {
                    return Err("invalid GGUF boolean".into());
                }
                Ok(Meta::Other)
            }
            2 | 3 => {
                self.bytes(2)?;
                Ok(Meta::Other)
            }
            4 => {
                let n = self.u32()?;
                Ok(if wanted { Meta::Uint(n) } else { Meta::Other })
            }
            5 | 6 => {
                self.bytes(4)?;
                Ok(Meta::Other)
            }
            10..=12 => {
                self.bytes(8)?;
                Ok(Meta::Other)
            }
            _ => Err("unknown GGUF metadata type".into()),
        }
    }
}
enum Meta {
    Text(String),
    Uint(u32),
    Strings(Vec<String>),
    Ints(Vec<i32>),
    Other,
}
struct Metadata {
    tokens: Vec<String>,
    types: Vec<i32>,
    merges: Vec<String>,
}
fn metadata(input: impl Read) -> Result<Metadata, String> {
    let mut r = Reader {
        inner: input,
        used: 0,
    };
    if r.bytes(4)? != b"GGUF" || r.u32()? != 3 {
        return Err("expected GGUF v3".into());
    }
    let tensors = r.u64()?;
    let count = r.u64()?;
    if tensors > MAX_ITEMS || count > 4096 {
        return Err("GGUF header count limit".into());
    }
    let mut entries = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for _ in 0..count {
        let key = r.string()?;
        if !seen.insert(key.clone()) {
            return Err("duplicate GGUF metadata key".into());
        }
        let kind = r.u32()?;
        let wanted = matches!(
            key.as_str(),
            "general.architecture"
                | "tokenizer.ggml.model"
                | "tokenizer.ggml.pre"
                | "tokenizer.ggml.tokens"
                | "tokenizer.ggml.token_type"
                | "tokenizer.ggml.merges"
                | "tokenizer.ggml.bos_token_id"
                | "tokenizer.ggml.eos_token_id"
                | "tokenizer.ggml.unknown_token_id"
        );
        let value = r.value(kind, wanted)?;
        if wanted {
            entries.insert(key, value);
        }
    }
    for (key, expected) in [
        ("general.architecture", "lfm3"),
        ("tokenizer.ggml.model", "gpt2"),
        ("tokenizer.ggml.pre", "lfm2"),
    ] {
        if !matches!(entries.remove(key), Some(Meta::Text(s)) if s == expected) {
            return Err(format!("invalid {key}"));
        }
    }
    for (key, expected) in [
        ("tokenizer.ggml.bos_token_id", 124_894),
        ("tokenizer.ggml.eos_token_id", 124_900),
    ] {
        if !matches!(entries.remove(key), Some(Meta::Uint(id)) if id == expected) {
            return Err(format!("invalid {key}"));
        }
    }
    if entries.contains_key("tokenizer.ggml.unknown_token_id") {
        return Err("unsupported unknown token configuration".into());
    }
    let tokens = match entries.remove("tokenizer.ggml.tokens") {
        Some(Meta::Strings(v)) => v,
        _ => return Err("missing GGUF tokens".into()),
    };
    let types = match entries.remove("tokenizer.ggml.token_type") {
        Some(Meta::Ints(v)) => v,
        _ => return Err("missing GGUF token types".into()),
    };
    let merges = match entries.remove("tokenizer.ggml.merges") {
        Some(Meta::Strings(v)) => v,
        _ => return Err("missing GGUF merges".into()),
    };
    if tokens.len() != 128000
        || types.len() != tokens.len()
        || types.iter().any(|x| !matches!(x, 1 | 3 | 4 | 5 | 6))
        || types.iter().filter(|x| **x == 4).count() != 14
    {
        return Err("invalid GGUF token counts or types".into());
    }
    for (kind, count) in [(1, 124_893), (3, 126), (4, 14), (5, 2_967)] {
        if types.iter().filter(|&&t| t == kind).count() != count {
            return Err("invalid GGUF token type distribution".into());
        }
    }
    if tokens.iter().collect::<BTreeSet<_>>().len() != tokens.len() {
        return Err("duplicate GGUF token".into());
    }
    Ok(Metadata {
        tokens,
        types,
        merges,
    })
}
fn validate_official_shape(json: &Value) -> Result<(), String> {
    let vocab = json
        .get("model")
        .and_then(|m| m.get("vocab"))
        .and_then(Value::as_object)
        .ok_or("missing tokenizer vocabulary")?;
    let added = json
        .get("added_tokens")
        .and_then(Value::as_array)
        .ok_or("missing added tokens")?;
    if vocab.len() != 124_893 || added.len() != 140 {
        return Err("invalid official vocabulary counts".into());
    }
    let mut ids = BTreeSet::new();
    for id in vocab.values().chain(added.iter().map(|v| &v["id"])) {
        let id = id.as_u64().ok_or("invalid official token ID")?;
        if id >= 125_033 || !ids.insert(id) {
            return Err("duplicate or invalid official token ID".into());
        }
    }
    if ids.len() != 125_033 {
        return Err("incomplete official token IDs".into());
    }
    Ok(())
}
fn canonical(v: Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, v) in m.into_iter().collect::<BTreeMap<_, _>>() {
                out.insert(k, canonical(v));
            }
            Value::Object(out)
        }
        Value::Array(v) => Value::Array(v.into_iter().map(canonical).collect()),
        x => x,
    }
}
fn transform(mut json: Value, meta: &Metadata) -> Result<Value, String> {
    let model = json
        .get("model")
        .and_then(Value::as_object)
        .ok_or("missing tokenizer model")?;
    if model.get("type").and_then(Value::as_str) != Some("BPE") {
        return Err("expected BPE tokenizer".into());
    }
    let vocab = model
        .get("vocab")
        .and_then(Value::as_object)
        .ok_or("missing tokenizer vocabulary")?;
    for (token, id) in vocab {
        let id = id.as_u64().ok_or("invalid vocabulary ID")? as usize;
        if meta.tokens.get(id) != Some(token) {
            return Err("official vocabulary ID mismatch".into());
        }
    }
    let added = json
        .get("added_tokens")
        .and_then(Value::as_array)
        .ok_or("missing added tokens")?;
    for entry in added {
        let id = entry
            .get("id")
            .and_then(Value::as_u64)
            .ok_or("invalid added token ID")? as usize;
        let token = entry
            .get("content")
            .and_then(Value::as_str)
            .ok_or("invalid added token")?;
        if meta.tokens.get(id).map(String::as_str) != Some(token) {
            return Err("official added token ID mismatch".into());
        }
    }
    let merges = model
        .get("merges")
        .and_then(Value::as_array)
        .ok_or("missing merges")?;
    if merges.len() != meta.merges.len() {
        return Err("merge count mismatch".into());
    }
    for (hf, gguf) in merges.iter().zip(&meta.merges) {
        let (a, b) = gguf.split_once(' ').ok_or("invalid GGUF merge")?;
        if a.is_empty()
            || b.is_empty()
            || hf.as_array().is_none_or(|v| {
                v.len() != 2 || v[0].as_str() != Some(a) || v[1].as_str() != Some(b)
            })
        {
            return Err("merge mismatch".into());
        }
    }
    let mut full = Map::new();
    for (i, token) in meta.tokens.iter().enumerate() {
        full.insert(token.clone(), json!(i));
    }
    let all_added: Vec<Value> = meta.tokens.iter().zip(&meta.types).enumerate().filter(|(_,(_,t))| **t != 1 && **t != 6).map(|(id,(token,t))| json!({"id":id,"content":token,"single_word":false,"lstrip":false,"rstrip":false,"normalized":false,"special": *t != 4})).collect();
    let root = json.as_object_mut().ok_or("invalid tokenizer JSON")?;
    root.insert("added_tokens".into(), Value::Array(all_added));
    root.insert("normalizer".into(), Value::Null);
    root.insert("pre_tokenizer".into(), json!({"type":"Sequence","pretokenizers":[{"type":"Split","pattern":{"Regex":REGEX},"behavior":"Isolated","invert":false},{"type":"ByteLevel","add_prefix_space":false,"trim_offsets":false,"use_regex":false}]}));
    for k in ["decoder", "post_processor"] {
        root.insert(k.into(), json!({"type":"ByteLevel","add_prefix_space":false,"trim_offsets":false,"use_regex":false}));
    }
    let model = root
        .get_mut("model")
        .and_then(Value::as_object_mut)
        .ok_or("missing tokenizer model")?;
    model.insert("vocab".into(), Value::Object(full));
    for key in [
        "dropout",
        "unk_token",
        "continuing_subword_prefix",
        "end_of_word_suffix",
    ] {
        model.insert(key.into(), Value::Null);
    }
    for key in ["fuse_unk", "byte_fallback"] {
        model.insert(key.into(), Value::Bool(false));
    }
    model.insert("ignore_merges".into(), Value::Bool(true));
    Ok(canonical(json))
}
fn open_input(path: &Path) -> Result<File, String> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(NOFOLLOW_NONBLOCK)
        .open(path)
        .map_err(|_| "input unavailable")?;
    if !f
        .metadata()
        .map_err(|_| "input metadata unavailable")?
        .is_file()
    {
        return Err("input is not a regular file".into());
    }
    Ok(f)
}
fn checked_directory(path: &Path) -> Result<(), String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|_| "current directory unavailable")?
            .join(path)
    };
    if absolute
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("unsafe output path".into());
    }
    let parent = absolute.parent().ok_or("output parent unavailable")?;
    if fs::canonicalize(parent).map_err(|_| "output parent unavailable")? != parent {
        return Err("output parent contains a symlink".into());
    }
    match fs::DirBuilder::new().mode(0o700).create(&absolute) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("output directory unavailable".into()),
    }
    let m = fs::symlink_metadata(&absolute).map_err(|_| "output directory unavailable")?;
    if !m.is_dir()
        || m.permissions().mode() & 0o777 != 0o700
        || fs::canonicalize(&absolute).map_err(|_| "output directory unavailable")? != absolute
    {
        return Err("output directory must be private and symlink-free".into());
    }
    Ok(())
}
/// Materialize a deterministic tokenizer, with the caller responsible for pinning inputs.
pub fn prepare_lfm3_tokenizer(
    gguf_path: &Path,
    official_tokenizer_path: &Path,
    output_dir: &Path,
) -> Result<PreparedTokenizer, String> {
    let meta = metadata(open_input(gguf_path)?)?;
    let f = open_input(official_tokenizer_path)?;
    if f.metadata()
        .map_err(|_| "tokenizer metadata unavailable")?
        .len()
        > MAX_JSON
    {
        return Err("tokenizer JSON limit".into());
    }
    let mut bytes = Vec::new();
    f.take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "tokenizer JSON unavailable")?;
    if bytes.len() as u64 > MAX_JSON {
        return Err("tokenizer JSON limit".into());
    }
    let input: Value = serde_json::from_slice(&bytes).map_err(|_| "invalid tokenizer JSON")?;
    validate_official_shape(&input)?;
    let output = serde_json::to_vec(&transform(input, &meta)?)
        .map_err(|_| "tokenizer serialization failed")?;
    let sha256 = format!("{:x}", Sha256::digest(&output));
    checked_directory(output_dir)?;
    let path = output_dir.join(format!("lfm3-{sha256}.json"));
    if path.exists() || fs::symlink_metadata(&path).is_ok() {
        let f = open_input(&path)?;
        let m = f.metadata().map_err(|_| "output metadata unavailable")?;
        if m.nlink() != 1
            || m.permissions().mode() & 0o777 != 0o600
            || m.len() != output.len() as u64
        {
            return Err("existing output mismatch".into());
        }
        let mut existing = Vec::new();
        f.take(MAX_JSON + 1)
            .read_to_end(&mut existing)
            .map_err(|_| "output unreadable")?;
        if existing != output {
            return Err("existing output mismatch".into());
        }
    } else {
        let temp = output_dir.join(format!(
            ".tokenizer-{}-{}.tmp",
            std::process::id(),
            &sha256[..16]
        ));
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(NOFOLLOW_NONBLOCK)
            .open(&temp)
            .map_err(|_| "temporary output unavailable")?;
        let result: Result<(), &str> = (|| {
            f.write_all(&output)
                .and_then(|()| f.sync_all())
                .map_err(|_| "output write failed")?;
            fs::hard_link(&temp, &path).map_err(|_| "output already exists or unavailable")?;
            Ok(())
        })();
        let removed = fs::remove_file(&temp);
        result?;
        removed.map_err(|_| "temporary output cleanup failed")?;
        File::open(output_dir)
            .and_then(|d| d.sync_all())
            .map_err(|_| "output directory sync failed")?;
    }
    Ok(PreparedTokenizer { path, sha256 })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> (Value, Metadata) {
        let meta = Metadata {
            tokens: vec!["a".into(), "b".into(), "<tool>".into(), "unused".into()],
            types: vec![1, 3, 4, 5],
            merges: vec!["a b".into()],
        };
        let hf = json!({"model":{"type":"BPE","vocab":{"a":0,"b":1},"merges":[["a","b"]]},"added_tokens":[{"id":2,"content":"<tool>"}]});
        (hf, meta)
    }
    #[test]
    fn malformed_metadata_is_bounded_and_rejected() {
        assert!(metadata(&b"GGUF\x03"[..]).is_err());
        let mut header = b"GGUF".to_vec();
        header.extend(3u32.to_le_bytes());
        header.extend(0u64.to_le_bytes());
        header.extend(1u64.to_le_bytes());
        let mut oversized = header.clone();
        oversized.extend((MAX_STRING + 1).to_le_bytes());
        assert!(metadata(&oversized[..]).is_err());
        let mut truncated = header.clone();
        truncated.extend(3u64.to_le_bytes());
        truncated.extend(b"a");
        assert!(metadata(&truncated[..]).is_err());
        let mut bad_array = header;
        bad_array.extend(1u64.to_le_bytes());
        bad_array.extend(b"x");
        bad_array.extend(9u32.to_le_bytes());
        bad_array.extend(8u32.to_le_bytes());
        bad_array.extend((MAX_ITEMS + 1).to_le_bytes());
        assert!(metadata(&bad_array[..]).is_err());
        let mut wrong_arch = b"GGUF".to_vec();
        wrong_arch.extend(3u32.to_le_bytes());
        wrong_arch.extend(0u64.to_le_bytes());
        wrong_arch.extend(1u64.to_le_bytes());
        let key = b"general.architecture";
        wrong_arch.extend((key.len() as u64).to_le_bytes());
        wrong_arch.extend(key);
        wrong_arch.extend(8u32.to_le_bytes());
        wrong_arch.extend(5u64.to_le_bytes());
        wrong_arch.extend(b"llama");
        assert!(matches!(metadata(&wrong_arch[..]), Err(e) if e.contains("architecture")));
        let mut wrong = b"GGUF".to_vec();
        wrong.extend(2u32.to_le_bytes());
        wrong.extend([0u8; 16]);
        assert!(metadata(&wrong[..]).is_err());
    }
    #[test]
    fn ids_merges_and_special_flags_are_checked() {
        let (hf, meta) = sample();
        let result = transform(hf.clone(), &meta).unwrap();
        let added = result["added_tokens"].as_array().unwrap();
        assert_eq!(added.len(), 3);
        assert_eq!(
            added
                .iter()
                .filter(|v| v["special"] == false)
                .map(|v| v["id"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![2]
        );
        let mut bad = hf.clone();
        bad["model"]["vocab"]["a"] = json!(1);
        assert!(transform(bad, &meta).is_err());
        let mut bad = hf.clone();
        bad["added_tokens"][0]["id"] = json!(3);
        assert!(transform(bad, &meta).is_err());
        let mut bad = hf;
        bad["model"]["merges"][0] = json!(["b", "a"]);
        assert!(transform(bad, &meta).is_err());
    }
    #[test]
    fn canonical_order_is_recursive() {
        let one: Value =
            serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"z":0,"a":1}]}"#).unwrap();
        let two: Value =
            serde_json::from_str(r#"{"a":[{"a":1,"z":0}],"z":{"a":1,"b":2}}"#).unwrap();
        assert_eq!(
            serde_json::to_vec(&canonical(one)).unwrap(),
            serde_json::to_vec(&canonical(two)).unwrap()
        );
    }
    #[test]
    fn output_directory_rejects_symlinks_and_nonprivate_modes() {
        use std::os::unix::fs::symlink;
        let base =
            std::env::temp_dir().join(format!("metron-tokenizer-perms-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        let actual = base.join("actual");
        checked_directory(&actual).unwrap();
        let alias = base.join("alias");
        symlink(&actual, &alias).unwrap();
        assert!(checked_directory(&alias).is_err());
        assert!(checked_directory(&alias.join("nested")).is_err());
        assert!(!actual.join("nested").exists());
        fs::set_permissions(&actual, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(checked_directory(&actual).is_err());
        fs::remove_dir_all(base).unwrap();
    }
    /// Run explicitly with the pinned local model and official tokenizer.
    #[test]
    #[ignore = "requires pinned local LFM3 GGUF and official tokenizer"]
    fn physical_model_matches_corrected_reference() {
        let gguf = std::env::var("METRON_LFM3_TOKENIZER_GGUF").expect("set GGUF path");
        let official =
            std::env::var("METRON_LFM3_OFFICIAL_TOKENIZER").expect("set official tokenizer path");
        let reference =
            std::env::var("METRON_LFM3_CORRECTED_REFERENCE").expect("set corrected reference path");
        let out =
            std::env::temp_dir().join(format!("metron-tokenizer-probe-{}", std::process::id()));
        let generated =
            prepare_lfm3_tokenizer(Path::new(&gguf), Path::new(&official), &out).unwrap();
        let actual: Value = serde_json::from_slice(&fs::read(&generated.path).unwrap()).unwrap();
        let expected: Value = serde_json::from_slice(&fs::read(reference).unwrap()).unwrap();
        assert_eq!(actual, expected);
        let added = actual["added_tokens"].as_array().unwrap();
        assert_eq!(added.len(), 3107);
        assert_eq!(added.iter().filter(|v| v["special"] == false).count(), 14);
        assert_eq!(
            generated.sha256,
            "2435fed9a91d7f8bc1c8744bf13f01fc210609592f2db7fbbbd4dbc30f46b9c9"
        );
        let again = prepare_lfm3_tokenizer(Path::new(&gguf), Path::new(&official), &out).unwrap();
        assert_eq!(generated.sha256, again.sha256);
        fs::write(&generated.path, b"tampered").unwrap();
        assert!(prepare_lfm3_tokenizer(Path::new(&gguf), Path::new(&official), &out).is_err());
        fs::remove_dir_all(out).unwrap();
    }
}
