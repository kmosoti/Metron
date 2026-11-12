//! JSON serialization with sorted maps and declaration-ordered structs.
use serde::ser::{self, Serialize, SerializeMap as _};

pub(crate) fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    value.serialize(Canonical)
}

struct Canonical;
type Result<T> = std::result::Result<T, serde_json::Error>;
fn scalar<T: Serialize + ?Sized>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v)
}
fn error(message: &str) -> serde_json::Error {
    <serde_json::Error as ser::Error>::custom(message)
}
fn object(entries: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>) -> Vec<u8> {
    let mut out = vec![b'{'];
    for (i, (key, value)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend(key);
        out.push(b':');
        out.extend(value);
    }
    out.push(b'}');
    out
}
fn variant(name: &str, value: Vec<u8>) -> Result<Vec<u8>> {
    Ok(object([(scalar(name)?, value)]))
}

struct Key<'a, T: ?Sized>(&'a T);
impl<T: Serialize + ?Sized> Serialize for Key<'_, T> {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.0, &())?;
        map.end()
    }
}
fn key<T: Serialize + ?Sized>(v: &T) -> Result<(String, Vec<u8>)> {
    let encoded = serde_json::to_vec(&Key(v))?;
    // A one-entry JSON object containing a null value ends in `:null}`.
    let bytes = encoded[1..encoded.len() - 6].to_vec();
    Ok((serde_json::from_slice(&bytes)?, bytes))
}

struct Sequence {
    values: Vec<Vec<u8>>,
    variant: Option<&'static str>,
}
impl Sequence {
    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.values.push(to_vec(value)?);
        Ok(())
    }
    fn finish(self) -> Result<Vec<u8>> {
        let mut out = vec![b'['];
        for (i, value) in self.values.into_iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.extend(value);
        }
        out.push(b']');
        if let Some(name) = self.variant {
            variant(name, out)
        } else {
            Ok(out)
        }
    }
}
macro_rules! sequence_impl {
    ($trait:ident, $method:ident) => {
        impl ser::$trait for Sequence {
            type Ok = Vec<u8>;
            type Error = serde_json::Error;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
                self.push(value)
            }
            fn end(self) -> Result<Vec<u8>> {
                self.finish()
            }
        }
    };
}
sequence_impl!(SerializeSeq, serialize_element);
sequence_impl!(SerializeTuple, serialize_element);
sequence_impl!(SerializeTupleStruct, serialize_field);
sequence_impl!(SerializeTupleVariant, serialize_field);

struct Map {
    entries: Vec<(String, Vec<u8>, Vec<u8>)>,
    pending: Option<(String, Vec<u8>)>,
}
impl ser::SerializeMap for Map {
    type Ok = Vec<u8>;
    type Error = serde_json::Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        if self.pending.is_some() {
            return Err(error("map key without value"));
        }
        self.pending = Some(key(value)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let (text, encoded) = self
            .pending
            .take()
            .ok_or_else(|| error("map value without key"))?;
        self.entries.push((text, encoded, to_vec(value)?));
        Ok(())
    }
    fn end(mut self) -> Result<Vec<u8>> {
        if self.pending.is_some() {
            return Err(error("map key without value"));
        }
        self.entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(object(self.entries.into_iter().map(|(_, k, v)| (k, v))))
    }
}
struct Struct {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    variant: Option<&'static str>,
}
impl Struct {
    fn field<T: Serialize + ?Sized>(&mut self, name: &'static str, value: &T) -> Result<()> {
        self.entries.push((scalar(name)?, to_vec(value)?));
        Ok(())
    }
    fn finish(self) -> Result<Vec<u8>> {
        let out = object(self.entries);
        if let Some(name) = self.variant {
            variant(name, out)
        } else {
            Ok(out)
        }
    }
}
impl ser::SerializeStruct for Struct {
    type Ok = Vec<u8>;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<()> {
        self.field(name, value)
    }
    fn end(self) -> Result<Vec<u8>> {
        self.finish()
    }
}
impl ser::SerializeStructVariant for Struct {
    type Ok = Vec<u8>;
    type Error = serde_json::Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        value: &T,
    ) -> Result<()> {
        self.field(name, value)
    }
    fn end(self) -> Result<Vec<u8>> {
        self.finish()
    }
}
macro_rules! scalars { ($($name:ident: $ty:ty),* $(,)?) => { $(fn $name(self, v: $ty) -> Result<Vec<u8>> { scalar(&v) })* }; }
impl ser::Serializer for Canonical {
    type Ok = Vec<u8>;
    type Error = serde_json::Error;
    type SerializeSeq = Sequence;
    type SerializeTuple = Sequence;
    type SerializeTupleStruct = Sequence;
    type SerializeTupleVariant = Sequence;
    type SerializeMap = Map;
    type SerializeStruct = Struct;
    type SerializeStructVariant = Struct;
    scalars!(serialize_bool: bool, serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64, serialize_i128: i128, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32, serialize_u64: u64, serialize_u128: u128, serialize_f32: f32, serialize_f64: f64, serialize_char: char);
    fn serialize_str(self, v: &str) -> Result<Vec<u8>> {
        scalar(v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Vec<u8>> {
        scalar(v)
    }
    fn serialize_none(self) -> Result<Vec<u8>> {
        Ok(b"null".to_vec())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Vec<u8>> {
        to_vec(v)
    }
    fn serialize_unit(self) -> Result<Vec<u8>> {
        Ok(b"null".to_vec())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Vec<u8>> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, v: &'static str) -> Result<Vec<u8>> {
        scalar(v)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<Vec<u8>> {
        to_vec(v)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        v: &T,
    ) -> Result<Vec<u8>> {
        variant(name, to_vec(v)?)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Sequence> {
        Ok(Sequence {
            values: vec![],
            variant: None,
        })
    }
    fn serialize_tuple(self, _: usize) -> Result<Sequence> {
        self.serialize_seq(None)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Sequence> {
        self.serialize_seq(None)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        _: usize,
    ) -> Result<Sequence> {
        Ok(Sequence {
            values: vec![],
            variant: Some(name),
        })
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Map> {
        Ok(Map {
            entries: vec![],
            pending: None,
        })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Struct> {
        Ok(Struct {
            entries: vec![],
            variant: None,
        })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        name: &'static str,
        _: usize,
    ) -> Result<Struct> {
        Ok(Struct {
            entries: vec![],
            variant: Some(name),
        })
    }
}
