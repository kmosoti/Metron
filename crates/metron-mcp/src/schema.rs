//! Pre-sandbox JSON Schema validation for MCP tool call arguments.

use crate::registry::McpBridgeError;
use rmcp::model::JsonObject;
use serde_json::{Map, Value};

/// Validates `args` against a tool's registered JSON Schema (`type`, `required`,
/// unknown fields when `additionalProperties: false` or `properties` is closed,
/// `enum`, `minLength`/`maxLength`, and `minimum`/`maximum`) BEFORE invoking the sandbox.
pub fn validate_args_against_schema(
    schema: &JsonObject,
    args: &Map<String, Value>,
) -> Result<(), McpBridgeError> {
    if let Some(ty) = schema.get("type").and_then(Value::as_str)
        && ty != "object"
    {
        return Err(McpBridgeError::InvalidArguments(format!(
            "tool schema root type must be object, got {ty}"
        )));
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for req in required {
            if let Some(field) = req.as_str()
                && !args.contains_key(field)
            {
                return Err(McpBridgeError::InvalidArguments(format!(
                    "missing required field '{field}'"
                )));
            }
        }
    }

    let properties = schema.get("properties").and_then(Value::as_object);
    let disallow_additional =
        matches!(schema.get("additionalProperties"), Some(Value::Bool(false)));

    for (key, value) in args {
        let Some(prop_schema) = properties.and_then(|p| p.get(key)) else {
            if disallow_additional {
                return Err(McpBridgeError::InvalidArguments(format!(
                    "unknown field '{key}' rejected by input_schema"
                )));
            }
            continue;
        };
        validate_value(key, value, prop_schema)?;
    }

    Ok(())
}

fn validate_value(key: &str, value: &Value, prop: &Value) -> Result<(), McpBridgeError> {
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
        if let Some(min_len) = obj.get("minLength").and_then(Value::as_u64)
            && (s.chars().count() as u64) < min_len
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' shorter than minLength {min_len}"
            )));
        }
        if let Some(max_len) = obj.get("maxLength").and_then(Value::as_u64)
            && (s.chars().count() as u64) > max_len
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' exceeds maxLength {max_len}"
            )));
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = obj.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' below minimum {min}"
            )));
        }
        if let Some(max) = obj.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(McpBridgeError::InvalidArguments(format!(
                "field '{key}' exceeds maximum {max}"
            )));
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
