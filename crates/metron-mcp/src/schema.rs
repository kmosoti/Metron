//! Pre-sandbox JSON Schema validation for MCP tool call arguments.

use crate::registry::McpBridgeError;
use rmcp::model::JsonObject;
use serde_json::{Map, Value};
use std::io::Write;

/// Maximum serialized JSON payload accepted for a single MCP tool invocation (1 MiB).
pub const MAX_MCP_ARGS_BYTES: usize = 1024 * 1024;
const MAX_SCHEMA_DEPTH: usize = 16;

struct BoundedVecWriter {
    buf: Vec<u8>,
    limit: usize,
}

impl Write for BoundedVecWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let next = self.buf.len().checked_add(data.len()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "args byte length overflow",
            )
        })?;
        if next > self.limit {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "MCP tool arguments exceed payload size limit",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes `args` while enforcing `max_bytes` before full payload allocation.
pub fn serialize_bounded_args(
    args: &Map<String, Value>,
    max_bytes: usize,
) -> Result<Vec<u8>, McpBridgeError> {
    let mut writer = BoundedVecWriter {
        buf: Vec::with_capacity(args.len().saturating_mul(32).min(4096)),
        limit: max_bytes,
    };
    if let Err(err) = serde_json::to_writer(&mut writer, args) {
        return Err(if err.is_io() {
            McpBridgeError::InvalidArguments(format!(
                "arguments exceed maximum payload size of {max_bytes} bytes"
            ))
        } else {
            McpBridgeError::Protocol(err.to_string())
        });
    }
    Ok(writer.buf)
}

/// Validates `args` against a tool's registered JSON Schema (`type`, `required`,
/// unknown fields when `additionalProperties: false`, nested `object`/`array` up to
/// `MAX_SCHEMA_DEPTH`, `enum`, `minLength`/`maxLength`, `minimum`/`maximum`) BEFORE
/// invoking the sandbox.
pub fn validate_args_against_schema(
    schema: &JsonObject,
    args: &Map<String, Value>,
) -> Result<(), McpBridgeError> {
    validate_object(schema, args, "$", 0)
}

fn validate_object(
    schema: &Map<String, Value>,
    args: &Map<String, Value>,
    path: &str,
    depth: usize,
) -> Result<(), McpBridgeError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(McpBridgeError::InvalidArguments(format!(
            "argument nesting at '{path}' exceeds maximum schema depth {MAX_SCHEMA_DEPTH}"
        )));
    }
    if let Some(ty) = schema.get("type").and_then(Value::as_str)
        && ty != "object"
    {
        return Err(McpBridgeError::InvalidArguments(format!(
            "tool schema at '{path}' expected type object, got {ty}"
        )));
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for req in required {
            if let Some(field) = req.as_str()
                && !args.contains_key(field)
            {
                return Err(McpBridgeError::InvalidArguments(format!(
                    "missing required field '{path}.{field}'"
                )));
            }
        }
    }

    let properties = schema.get("properties").and_then(Value::as_object);
    let disallow_additional =
        matches!(schema.get("additionalProperties"), Some(Value::Bool(false)));

    for (key, value) in args {
        let child_path = format!("{path}.{key}");
        let Some(prop_schema) = properties.and_then(|p| p.get(key)) else {
            if disallow_additional {
                return Err(McpBridgeError::InvalidArguments(format!(
                    "unknown field '{child_path}' rejected by input_schema"
                )));
            }
            continue;
        };
        validate_value(&child_path, value, prop_schema, depth + 1)?;
    }

    Ok(())
}

fn validate_value(
    key: &str,
    value: &Value,
    prop: &Value,
    depth: usize,
) -> Result<(), McpBridgeError> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(McpBridgeError::InvalidArguments(format!(
            "field '{key}' exceeds maximum nesting depth {MAX_SCHEMA_DEPTH}"
        )));
    }
    let Some(obj) = prop.as_object() else {
        return Ok(());
    };
    if let Some(expected_type) = obj.get("type").and_then(Value::as_str)
        && !matches_json_type(value, expected_type)
    {
        return Err(McpBridgeError::InvalidArguments(format!(
            "field '{key}' expected type '{expected_type}'"
        )));
    }
    if let Some(allowed) = obj.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(McpBridgeError::InvalidArguments(format!(
            "field '{key}' value not in schema enum"
        )));
    }
    if let Some(s) = value.as_str() {
        let len = s.chars().count() as u64;
        if let Some(min_len) = obj.get("minLength").and_then(Value::as_u64)
            && len < min_len
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' shorter than minLength {min_len}"
            )));
        }
        if let Some(max_len) = obj.get("maxLength").and_then(Value::as_u64)
            && len > max_len
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' exceeds maxLength {max_len}"
            )));
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = obj.get("minimum").and_then(Value::as_f64)
            && (!n.is_finite() || n < min)
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' below minimum {min}"
            )));
        }
        if let Some(max) = obj.get("maximum").and_then(Value::as_f64)
            && (!n.is_finite() || n > max)
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' exceeds maximum {max}"
            )));
        }
    }
    if let Some(nested_obj) = value.as_object()
        && (obj.contains_key("properties")
            || obj.contains_key("required")
            || obj.contains_key("additionalProperties"))
    {
        validate_object(obj, nested_obj, key, depth)?;
    }
    if let Some(arr) = value.as_array() {
        let count = arr.len() as u64;
        if let Some(min_items) = obj.get("minItems").and_then(Value::as_u64)
            && count < min_items
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' has fewer than minItems {min_items}"
            )));
        }
        if let Some(max_items) = obj.get("maxItems").and_then(Value::as_u64)
            && count > max_items
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' exceeds maxItems {max_items}"
            )));
        }
        if let Some(item_schema) = obj.get("items") {
            for (idx, item) in arr.iter().enumerate() {
                validate_value(&format!("{key}[{idx}]"), item, item_schema, depth + 1)?;
            }
        }
    }
    Ok(())
}

fn matches_json_type(value: &Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_object_array_and_depth_cap_are_enforced() {
        let schema = match json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "required": ["mode"],
                    "properties": {
                        "mode": { "type": "string", "enum": ["fast", "strict"] },
                        "items": { "type": "array", "maxItems": 2, "items": { "type": "integer", "minimum": 1 } }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }) {
            Value::Object(map) => map,
            _ => unreachable!(),
        };

        let ok = match json!({"nested": {"mode": "fast", "items": [1, 2]}}) {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        assert!(validate_args_against_schema(&schema, &ok).is_ok());

        let bad_item = match json!({"nested": {"mode": "fast", "items": [0]}}) {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        assert!(matches!(
            validate_args_against_schema(&schema, &bad_item),
            Err(McpBridgeError::InvalidArguments(_))
        ));

        let extra_nested = match json!({"nested": {"mode": "fast", "extra": 1}}) {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        assert!(matches!(
            validate_args_against_schema(&schema, &extra_nested),
            Err(McpBridgeError::InvalidArguments(_))
        ));
    }

    #[test]
    fn serialize_bounded_args_rejects_oversized_payload() {
        let mut args = Map::new();
        args.insert("blob".to_owned(), Value::String("a".repeat(256)));
        assert!(matches!(
            serialize_bounded_args(&args, 64),
            Err(McpBridgeError::InvalidArguments(_))
        ));
        assert!(serialize_bounded_args(&args, 1024).is_ok());
    }
}
