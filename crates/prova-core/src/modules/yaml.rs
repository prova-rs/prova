use mlua::{Lua, LuaSerdeExt, Table};
use serde::Deserialize;

use super::format_names;

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let yaml = lua.create_table()?;

    // yaml.decode(text) → Lua value for the single/first document. Raises on invalid YAML.
    yaml.set(
        format_names::DECODE,
        lua.create_function(|lua, text: String| {
            let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)
                .map_err(|e| mlua::Error::RuntimeError(format!("yaml.decode: {e}")))?;
            lua.to_value(&value)
        })?,
    )?;

    // yaml.decode_all(text) → list of Lua values, one per `---`-separated document. Raises on the
    // first invalid document (with its 1-based index). An empty/whitespace-only string yields {}.
    yaml.set(
        format_names::DECODE_ALL,
        lua.create_function(|lua, text: String| {
            let out = lua.create_table()?;
            for (i, doc) in serde_yaml_ng::Deserializer::from_str(&text).enumerate() {
                let value = serde_yaml_ng::Value::deserialize(doc).map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "yaml.decode_all: document {}: {e}",
                        i + 1
                    ))
                })?;
                out.push(lua.to_value(&value)?)?;
            }
            Ok(out)
        })?,
    )?;

    // yaml.encode(v) → YAML text for one document. Carries the json sentinels (api-freeze §1):
    // `json.null` emits an explicit null, `json.array{}` forces a flow-empty sequence.
    yaml.set(
        format_names::ENCODE,
        lua.create_function(|lua, v: mlua::Value| {
            let jv = super::formats::lua_value_to_json(lua, &v)?;
            serde_yaml_ng::to_string(&jv)
                .map_err(|e| mlua::Error::RuntimeError(format!("yaml.encode: {e}")))
        })?,
    )?;

    // yaml.encode_all(docs) → one `---`-separated stream (the k8s manifest shape), the exact
    // inverse of decode_all.
    yaml.set(
        format_names::ENCODE_ALL,
        lua.create_function(|lua, docs: Table| {
            let mut out = String::new();
            for doc in docs.sequence_values::<mlua::Value>() {
                let jv = super::formats::lua_value_to_json(lua, &doc?)?;
                if !out.is_empty() {
                    out.push_str("---\n");
                }
                out.push_str(&serde_yaml_ng::to_string(&jv).map_err(|e| {
                    mlua::Error::RuntimeError(format!("yaml.encode_all: {e}"))
                })?);
            }
            Ok(out)
        })?,
    )?;

    Ok(yaml)
}
