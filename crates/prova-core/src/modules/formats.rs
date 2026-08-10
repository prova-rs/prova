//! The document-format and encoding codecs: `prova.parse.*`, `json`, `toml`, `csv`,
//! `base64`, `hash`, `uuid`, `url` — plus the Lua<->JSON value converters they and the
//! feature-gated formats (`yaml`) share. One naming rule for all of them: see `format_names`
//! in the parent module.

use mlua::{Lua, LuaSerdeExt, Table, Value};

/// `prova.parse.*` — the exec-CLI output-parsing toolkit. A docker-exec plugin drives a CLI and gets
/// text back; these turn the common shapes into Lua values, so plugins never hand-roll parsing:
/// `lines` (line-oriented), `rows`/`table` (delimited — TSV/psql `|`). Format-*specific* parsing
/// lives in the tech-first modules (`json`, `yaml`, `toml`, `csv`) — `prova.parse.json` was removed
/// in the api-freeze §1 clean break.
pub(crate) fn make_parse(lua: &Lua) -> mlua::Result<Table> {
    let parse = lua.create_table()?;

    // lines(s) → non-empty, trimmed lines.
    parse.set(
        "lines",
        lua.create_function(|lua, s: String| {
            let out: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
            lua.create_sequence_from(out)
        })?,
    )?;

    // rows(s, sep?) → a list of rows, each a list of columns split on `sep` (default tab). Blank
    // lines are skipped.
    parse.set(
        "rows",
        lua.create_function(|lua, (s, sep): (String, Option<String>)| {
            let sep = sep.unwrap_or_else(|| "\t".to_string());
            let rows = lua.create_table()?;
            for (i, line) in s.lines().filter(|l| !l.is_empty()).enumerate() {
                rows.set(i + 1, lua.create_sequence_from(line.split(&sep))?)?;
            }
            Ok(rows)
        })?,
    )?;

    // table(s, sep?) → the first non-empty line is a header row; each remaining row becomes a map
    // keyed by header name (the "column by header" shape, e.g. rabbitmqadmin's TSV).
    parse.set(
        "table",
        lua.create_function(|lua, (s, sep): (String, Option<String>)| {
            let sep = sep.unwrap_or_else(|| "\t".to_string());
            let mut non_empty = s.lines().filter(|l| !l.is_empty());
            let headers: Vec<&str> = match non_empty.next() {
                Some(h) => h.split(&sep).collect(),
                None => return lua.create_table(),
            };
            let rows = lua.create_table()?;
            for (i, line) in non_empty.enumerate() {
                let cols: Vec<&str> = line.split(&sep).collect();
                let row = lua.create_table()?;
                for (j, h) in headers.iter().enumerate() {
                    row.set(*h, *cols.get(j).unwrap_or(&""))?;
                }
                rows.set(i + 1, row)?;
            }
            Ok(rows)
        })?,
    )?;

    Ok(parse)
}

/// Convert a `serde_json::Value` to a Lua value, mapping JSON `null` to Lua `nil` (so an absent
/// field reads as nil, not a null sentinel).
pub(crate) fn json_value_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<mlua::Value> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Nil,
        J::Bool(b) => Value::Boolean(*b),
        J::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(0.0)),
        },
        J::String(s) => Value::String(lua.create_string(s)?),
        J::Array(a) => {
            let t = lua.create_table()?;
            for (i, item) in a.iter().enumerate() {
                t.set(i + 1, json_value_to_lua(lua, item)?)?;
            }
            Value::Table(t)
        }
        J::Object(o) => {
            let t = lua.create_table()?;
            for (k, val) in o {
                t.set(k.as_str(), json_value_to_lua(lua, val)?)?;
            }
            Value::Table(t)
        }
    })
}

/// Convert a Lua value to a `serde_json::Value` — the encode half shared by `json.encode`,
/// `yaml.encode`, and `toml.encode`, carrying the fidelity sentinels (api-freeze §1):
///
/// - `json.null` (mlua's null lightuserdata) encodes as explicit `null`;
/// - a table wearing the array metatable (`json.array{...}`) is an array even when empty;
/// - a bare empty table is an **object** (`{}` — the common case for JSON APIs), a table with
///   sequence entries is an array, anything else is an object with stringified keys.
pub(crate) fn lua_value_to_json(lua: &Lua, v: &Value) -> mlua::Result<serde_json::Value> {
    use serde_json::Value as J;
    Ok(match v {
        Value::Nil => J::Null,
        Value::Boolean(b) => J::Bool(*b),
        Value::Integer(i) => J::Number((*i).into()),
        Value::Number(n) => serde_json::Number::from_f64(*n).map(J::Number).ok_or_else(|| {
            mlua::Error::RuntimeError(format!("cannot encode non-finite number {n}"))
        })?,
        Value::String(s) => J::String(s.to_str()?.to_string()),
        Value::LightUserData(l) if l.0.is_null() => J::Null,
        Value::Table(t) => {
            let is_array = t
                .metatable()
                .is_some_and(|mt| mt == lua.array_metatable());
            if is_array || t.raw_len() > 0 {
                let mut out = Vec::with_capacity(t.raw_len());
                for item in t.clone().sequence_values::<Value>() {
                    out.push(lua_value_to_json(lua, &item?)?);
                }
                J::Array(out)
            } else {
                let mut out = serde_json::Map::new();
                for pair in t.clone().pairs::<Value, Value>() {
                    let (k, val) = pair?;
                    let key = match &k {
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        other => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "cannot encode table key of type {}",
                                other.type_name()
                            )))
                        }
                    };
                    out.insert(key, lua_value_to_json(lua, &val)?);
                }
                J::Object(out)
            }
        }
        other => {
            return Err(mlua::Error::RuntimeError(format!(
                "cannot encode a {} value",
                other.type_name()
            )))
        }
    })
}

// ---------------------------------------------------------------------------------------------
// json / toml / csv — the tech-first format modules (api-freeze §1): decode AND encode together
// ---------------------------------------------------------------------------------------------

/// `json.*` — decode/encode plus the fidelity sentinels: `json.null` asserts/emits an explicit
/// null (decode's ergonomic default maps null → nil); `json.array` forces `[]` for empty or
/// ambiguous tables (a bare `{}` encodes as an object).
pub(crate) fn make_json(lua: &Lua) -> mlua::Result<Table> {
    let json = lua.create_table()?;

    // json.decode(s) → Lua value. JSON null → nil, top-level or nested (the ergonomic default:
    // `t:expect(v.x):is_nil()` must hold for a null field).
    json.set(
        "decode",
        lua.create_function(|lua, s: String| {
            let v: serde_json::Value = serde_json::from_str(&s)
                .map_err(|e| mlua::Error::RuntimeError(format!("json.decode: {e}")))?;
            json_value_to_lua(lua, &v)
        })?,
    )?;

    // json.encode(v, opts?) → compact JSON text (`opts.pretty = true` for indented).
    json.set(
        "encode",
        lua.create_function(|lua, (v, opts): (Value, Option<Table>)| {
            let jv = lua_value_to_json(lua, &v)?;
            let pretty = opts
                .map(|o| o.get::<Option<bool>>("pretty"))
                .transpose()?
                .flatten()
                .unwrap_or(false);
            let out = if pretty {
                serde_json::to_string_pretty(&jv)
            } else {
                serde_json::to_string(&jv)
            };
            out.map_err(|e| mlua::Error::RuntimeError(format!("json.encode: {e}")))
        })?,
    )?;

    // json.null — the explicit-null sentinel (assert it in shapes, emit it from encode).
    json.set("null", lua.null())?;

    // json.array(t) — mark `t` as an array for encoding (forces `[]` when empty).
    json.set(
        "array",
        lua.create_function(|lua, t: Table| {
            t.set_metatable(Some(lua.array_metatable()))?;
            Ok(t)
        })?,
    )?;

    Ok(json)
}

/// `toml.*` — decode/encode, exposing the dep the manifest reader already compiles in.
pub(crate) fn make_toml(lua: &Lua) -> mlua::Result<Table> {
    let toml_ns = lua.create_table()?;

    // toml.decode(s) → Lua value. Raises on invalid TOML.
    toml_ns.set(
        super::format_names::DECODE,
        lua.create_function(|lua, s: String| {
            let v: toml::Value = toml::from_str(&s)
                .map_err(|e| mlua::Error::RuntimeError(format!("toml.decode: {e}")))?;
            lua.to_value(&v)
        })?,
    )?;

    // toml.encode(v) → TOML text. The value must be table-shaped at the root (TOML documents are
    // tables); TOML has no null, so `json.null` is an encode error here.
    toml_ns.set(
        "encode",
        lua.create_function(|lua, v: Value| {
            let jv = lua_value_to_json(lua, &v)?;
            toml::to_string(&jv)
                .map_err(|e| mlua::Error::RuntimeError(format!("toml.encode: {e}")))
        })?,
    )?;

    Ok(toml_ns)
}

/// `csv.*` — header-aware decode/encode; the row shape mirrors `prova.parse.table` (a list of
/// header-keyed maps, every value a string — CSV is untyped text).
pub(crate) fn make_csv(lua: &Lua) -> mlua::Result<Table> {
    let csv_ns = lua.create_table()?;

    // csv.decode(s, opts?) → { {header = value, ...}, ... }. `opts.delimiter` (default ",").
    csv_ns.set(
        super::format_names::DECODE,
        lua.create_function(|lua, (s, opts): (String, Option<Table>)| {
            let delimiter = csv_delimiter(&opts, "csv.decode")?;
            let mut reader = csv::ReaderBuilder::new()
                .delimiter(delimiter)
                .from_reader(s.as_bytes());
            let headers = reader
                .headers()
                .map_err(|e| mlua::Error::RuntimeError(format!("csv.decode: {e}")))?
                .clone();
            let rows = lua.create_table()?;
            for (i, record) in reader.records().enumerate() {
                let record =
                    record.map_err(|e| mlua::Error::RuntimeError(format!("csv.decode: {e}")))?;
                let row = lua.create_table()?;
                for (h, field) in headers.iter().zip(record.iter()) {
                    row.set(h, field)?;
                }
                rows.set(i + 1, row)?;
            }
            Ok(rows)
        })?,
    )?;

    // csv.encode(rows, opts?) → CSV text with a header line. Column order: `opts.headers` when
    // given, else the first row's keys sorted (Lua table order is nondeterministic; sorted output
    // is diffable). Quoting is automatic (RFC 4180).
    csv_ns.set(
        "encode",
        lua.create_function(|_, (rows, opts): (Table, Option<Table>)| {
            let mut headers: Vec<String> = Vec::new();
            if let Some(hs) = opts
                .as_ref()
                .map(|o| o.get::<Option<Table>>("headers"))
                .transpose()?
                .flatten()
            {
                for h in hs.sequence_values::<String>() {
                    headers.push(h?);
                }
            } else if let Some(first) = rows.get::<Option<Table>>(1)? {
                for pair in first.pairs::<String, Value>() {
                    headers.push(pair?.0);
                }
                headers.sort();
            }
            let delimiter = csv_delimiter(&opts, "csv.encode")?;
            let mut writer = csv::WriterBuilder::new()
                .delimiter(delimiter)
                .from_writer(Vec::new());
            let fail = |e: csv::Error| mlua::Error::RuntimeError(format!("csv.encode: {e}"));
            writer.write_record(&headers).map_err(fail)?;
            for row in rows.sequence_values::<Table>() {
                let row = row?;
                let mut record = Vec::with_capacity(headers.len());
                for h in &headers {
                    record.push(match row.get::<Value>(h.as_str())? {
                        Value::Nil => String::new(),
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Boolean(b) => b.to_string(),
                        other => {
                            return Err(mlua::Error::RuntimeError(format!(
                                "csv.encode: cannot encode a {} field",
                                other.type_name()
                            )))
                        }
                    });
                }
                writer.write_record(&record).map_err(fail)?;
            }
            let bytes = writer
                .into_inner()
                .map_err(|e| mlua::Error::RuntimeError(format!("csv.encode: {e}")))?;
            String::from_utf8(bytes)
                .map_err(|e| mlua::Error::RuntimeError(format!("csv.encode: {e}")))
        })?,
    )?;

    Ok(csv_ns)
}

/// The one-byte `delimiter` option shared by `csv.decode` / `csv.encode`.
fn csv_delimiter(opts: &Option<Table>, who: &str) -> mlua::Result<u8> {
    let Some(d) = opts
        .as_ref()
        .map(|o| o.get::<Option<String>>("delimiter"))
        .transpose()?
        .flatten()
    else {
        return Ok(b',');
    };
    match d.as_bytes() {
        [b] => Ok(*b),
        _ => Err(mlua::Error::RuntimeError(format!(
            "{who}: delimiter must be a single byte, got {d:?}"
        ))),
    }
}

// ---------------------------------------------------------------------------------------------
// base64 / hash / uuid / url — the utility belt (api-freeze §1)
// ---------------------------------------------------------------------------------------------

/// `base64.{encode,decode}` — standard alphabet with padding, binary-safe in both directions.
pub(crate) fn make_base64(lua: &Lua) -> mlua::Result<Table> {
    use base64::Engine as _;
    let b64 = lua.create_table()?;
    b64.set(
        "encode",
        lua.create_function(|_, s: mlua::String| {
            Ok(base64::engine::general_purpose::STANDARD.encode(s.as_bytes()))
        })?,
    )?;
    b64.set(
        "decode",
        lua.create_function(|lua, s: String| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s.as_bytes())
                .map_err(|e| mlua::Error::RuntimeError(format!("base64.decode: {e}")))?;
            lua.create_string(&bytes)
        })?,
    )?;
    Ok(b64)
}

/// `hash.{sha256,hmac_sha256}` — lowercase hex digests.
pub(crate) fn make_hash(lua: &Lua) -> mlua::Result<Table> {
    use hmac::Mac as _;
    use sha2::Digest as _;
    let hash = lua.create_table()?;
    hash.set(
        "sha256",
        lua.create_function(|_, s: mlua::String| {
            Ok(hex_string(&sha2::Sha256::digest(s.as_bytes())))
        })?,
    )?;
    hash.set(
        "hmac_sha256",
        lua.create_function(|_, (key, msg): (mlua::String, mlua::String)| {
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key.as_bytes())
                .expect("HMAC accepts any key length");
            mac.update(&msg.as_bytes());
            Ok(hex_string(&mac.finalize().into_bytes()))
        })?,
    )?;
    Ok(hash)
}

fn hex_string(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// `uuid.v4()` — a random RFC 4122 id, hyphenated lowercase.
pub(crate) fn make_uuid(lua: &Lua) -> mlua::Result<Table> {
    let uuid_ns = lua.create_table()?;
    uuid_ns.set(
        "v4",
        lua.create_function(|_, ()| Ok(uuid::Uuid::new_v4().to_string()))?,
    )?;
    Ok(uuid_ns)
}

/// `url.{parse,encode}` — structured URL parts, and RFC 3986 component percent-encoding.
pub(crate) fn make_url(lua: &Lua) -> mlua::Result<Table> {
    let url_ns = lua.create_table()?;

    // url.parse(s) → { scheme, host?, port?, path, query?, fragment? }. `port` falls back to the
    // scheme's well-known default (an http probe wants a port, written or implied).
    url_ns.set(
        "parse",
        lua.create_function(|lua, s: String| {
            let u = url::Url::parse(&s)
                .map_err(|e| mlua::Error::RuntimeError(format!("url.parse: {e}")))?;
            let t = lua.create_table()?;
            t.set("scheme", u.scheme())?;
            if let Some(host) = u.host_str() {
                t.set("host", host)?;
            }
            if let Some(port) = u.port_or_known_default() {
                t.set("port", port)?;
            }
            t.set("path", u.path())?;
            if let Some(q) = u.query() {
                t.set("query", q)?;
            }
            if let Some(f) = u.fragment() {
                t.set("fragment", f)?;
            }
            Ok(t)
        })?,
    )?;

    // url.encode(s) → the string percent-encoded as one component: everything but RFC 3986
    // unreserved characters is escaped (space → %20 — form_urlencoded's `+` is the wrong shape
    // for a path or query component).
    const COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    url_ns.set(
        "encode",
        lua.create_function(|_, s: String| {
            Ok(percent_encoding::utf8_percent_encode(&s, COMPONENT).to_string())
        })?,
    )?;

    Ok(url_ns)
}
