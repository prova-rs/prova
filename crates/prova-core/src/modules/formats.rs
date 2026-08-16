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
            // Wear the array metatable, so an array that came IN as an array goes back out as
            // one. Without it a decoded `[]` is indistinguishable from a decoded `{}` — both are
            // bare empty tables — and re-encoding silently turns the array into an object. That
            // is a data-shape change at a boundary, the same class of quiet wrong as a lossy body
            // conversion, and many APIs treat `[]` and `{}` as different requests.
            //
            // Behaviorally transparent to Lua: the metatable is a pure marker, so `#`, `pairs`,
            // `ipairs` and indexing are unchanged, and `values_equal`/`subset_mismatch` compare
            // structure rather than metatables — a decoded array still `equals` a plain literal.
            t.set_metatable(Some(lua.array_metatable()))?;
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
            // Duplicate headers cannot be represented as a header-keyed map: the second column
            // overwrites the first, so a two-column file silently becomes a one-key row and the
            // dropped column is never mentioned. Refuse rather than lose data — the row shape is
            // the whole contract of this verb, and it is unsatisfiable here.
            let mut seen = std::collections::BTreeSet::new();
            for h in headers.iter() {
                if !seen.insert(h) {
                    return Err(mlua::Error::RuntimeError(format!(
                        "csv.decode: duplicate header {h:?} — rows are header-keyed maps, so two                          columns of the same name cannot both survive. Rename one, or read the                          file positionally."
                    )));
                }
            }
            // A list of rows, even when there are none — see `modules::list_table`.
            let rows = super::list_table(lua, Vec::<mlua::Value>::new())?;
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
            let headers = csv_headers(&rows, &opts)?;
            if headers.is_empty() {
                return Ok(String::new());
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

/// Which columns `csv.encode` writes, and the guarantee that comes with each way of deciding.
///
/// DECLARED (`opts.headers`) is the author naming the columns, so a row field left out is a
/// projection they asked for. DERIVED (the first row's keys, sorted for a diffable order) is a
/// GUESS, and a later row carrying a key the first one lacks would be dropped without a word —
/// data loss decided by which row happened to be first. So a derived set must fit every row, and
/// says so when it does not.
///
/// Empty means there is nothing to describe: no declared headers and no first row. Writing a
/// record for that emits a header line declaring one column named the empty string, so the caller
/// returns early instead.
fn csv_headers(rows: &Table, opts: &Option<Table>) -> mlua::Result<Vec<String>> {
    let declared = opts
        .as_ref()
        .map(|o| o.get::<Option<Table>>("headers"))
        .transpose()?
        .flatten();
    let was_declared = declared.is_some();

    let mut headers: Vec<String> = Vec::new();
    if let Some(hs) = declared {
        for h in hs.sequence_values::<String>() {
            headers.push(h?);
        }
    } else if let Some(first) = rows.get::<Option<Table>>(1)? {
        for pair in first.pairs::<String, Value>() {
            headers.push(pair?.0);
        }
        headers.sort();
    }
    if was_declared || headers.is_empty() {
        return Ok(headers);
    }

    let known: std::collections::BTreeSet<&str> = headers.iter().map(String::as_str).collect();
    for (i, row) in rows.clone().sequence_values::<Table>().enumerate() {
        for pair in row?.pairs::<String, Value>() {
            let key = pair?.0;
            if !known.contains(key.as_str()) {
                return Err(mlua::Error::RuntimeError(format!(
                    "csv.encode: row {} has a `{key}` field, which the header row does not \u{2014} \
                     headers come from the FIRST row, so this column would be dropped without a \
                     word. Pass `headers = {{ \u{2026} }}` to name the columns you want.",
                    i + 1
                )));
            }
        }
    }
    Ok(headers)
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
                .map_err(|e| mlua::Error::RuntimeError(format!("hash.hmac_sha256: {e}")))?;
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

    // url.decode(s) → `encode`'s exact inverse. An encoder without one left a proof that RECEIVES
    // a percent-encoded value — a redirect's Location, a query parameter, a header — with nothing
    // to read it but a hand-rolled decoder, which is the well-known place to introduce a quiet bug
    // that this module declares a crate to avoid.
    //
    // Component decoding, not form decoding: `+` stays a literal plus, because `encode` emits
    // `%20` for a space. The two conventions disagree on exactly one character, and picking the
    // wrong one silently corrupts every value containing it.
    //
    // Bytes, not lossy text: a percent sequence can encode any octet, and turning an invalid one
    // into U+FFFD is the same silent corruption `res.body` was fixed for. A Lua string is a byte
    // string, so the exact octets cross unharmed.
    url_ns.set(
        "decode",
        lua.create_function(|lua, s: String| {
            lua.create_string(percent_encoding::percent_decode_str(&s).collect::<Vec<u8>>())
        })?,
    )?;

    Ok(url_ns)
}

#[cfg(test)]
mod fidelity_tests {
    use super::*;
    use serde_json::json;

    fn lua() -> Lua {
        Lua::new()
    }

    fn to_json(l: &Lua, v: Value) -> mlua::Result<serde_json::Value> {
        lua_value_to_json(l, &v)
    }

    /// The empty-table question has no right answer, only a chosen one — and the choice is
    /// load-bearing, because a JSON API asserted against `{}` and one asserted against `[]` are
    /// different assertions. `{}` is an OBJECT (the common case for APIs); `json.array{}` is how
    /// you say the other thing.
    #[test]
    fn an_empty_table_is_an_object_and_the_array_sentinel_overrides_it() {
        let l = lua();
        let bare = l.create_table().unwrap();
        assert_eq!(to_json(&l, Value::Table(bare)).unwrap(), json!({}));

        let arr = l.create_table().unwrap();
        arr.set_metatable(Some(l.array_metatable())).unwrap();
        assert_eq!(
            to_json(&l, Value::Table(arr)).unwrap(),
            json!([]),
            "the sentinel wins over the empty-table default"
        );
    }

    /// A table with sequence entries is an array; one with named keys is an object. Getting this
    /// backwards would silently reshape every payload a proof sends.
    #[test]
    fn shape_follows_the_table_and_keys_are_stringified() {
        let l = lua();
        let seq = l.create_table().unwrap();
        seq.set(1, "a").unwrap();
        seq.set(2, "b").unwrap();
        assert_eq!(to_json(&l, Value::Table(seq)).unwrap(), json!(["a", "b"]));

        let map = l.create_table().unwrap();
        map.set("name", "prova").unwrap();
        assert_eq!(to_json(&l, Value::Table(map)).unwrap(), json!({"name": "prova"}));

        // A non-sequential integer key cannot be an array index, so it becomes an object key —
        // stringified, since JSON has no integer keys.
        let sparse = l.create_table().unwrap();
        sparse.set(7, "seven").unwrap();
        assert_eq!(to_json(&l, Value::Table(sparse)).unwrap(), json!({"7": "seven"}));
    }

    /// `nil` and the null sentinel both encode as JSON null, but they are NOT the same thing on
    /// the way back: decode maps null → nil so `t:expect(body.field):is_nil()` holds, which is why
    /// the sentinel exists for the encode direction at all.
    #[test]
    fn null_crosses_in_both_directions_without_becoming_a_string() {
        let l = lua();
        assert_eq!(to_json(&l, Value::Nil).unwrap(), json!(null));
        let sentinel = Value::LightUserData(mlua::LightUserData(std::ptr::null_mut()));
        assert_eq!(to_json(&l, sentinel).unwrap(), json!(null));

        // Decode: null becomes nil, not the string "null" and not a userdata the author must know
        // to compare against.
        let back = json_value_to_lua(&l, &json!(null)).unwrap();
        assert!(matches!(back, Value::Nil), "null decodes to nil, got {back:?}");
    }

    /// Integers must not silently become floats: an id of 9007199254740993 that round-trips as
    /// 9007199254740992.0 is a payload that looks right and addresses the wrong row.
    #[test]
    fn integers_stay_integers_and_floats_stay_floats() {
        let l = lua();
        assert_eq!(to_json(&l, Value::Integer(42)).unwrap(), json!(42));
        assert!(to_json(&l, Value::Integer(42)).unwrap().is_i64());

        let f = to_json(&l, Value::Number(1.5)).unwrap();
        assert!(f.is_f64() && (f.as_f64().unwrap() - 1.5).abs() < f64::EPSILON);

        // And back: a JSON integer arrives as a Lua integer, not a float.
        assert!(matches!(
            json_value_to_lua(&l, &json!(7)).unwrap(),
            Value::Integer(7)
        ));
    }

    /// JSON has no infinity or NaN. Encoding one must RAISE rather than emit something a parser
    /// will reject far away, or worse, coerce to a number that is merely wrong.
    #[test]
    fn a_non_finite_number_is_refused_rather_than_coerced() {
        let l = lua();
        for bad in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let err = to_json(&l, Value::Number(bad)).unwrap_err().to_string();
            assert!(err.contains("non-finite"), "named for what it is: {err}");
        }
    }

    /// A value JSON cannot represent is an error at the boundary, where the author's own call
    /// site is still on the stack — not a silently dropped field discovered by the server.
    #[test]
    fn an_unencodable_value_or_key_is_named_at_the_boundary() {
        let l = lua();
        let f = l.create_function(|_, ()| Ok(())).unwrap();
        let err = to_json(&l, Value::Function(f.clone())).unwrap_err().to_string();
        assert!(err.contains("cannot encode"), "{err}");

        let t = l.create_table().unwrap();
        t.set(f, "value").unwrap();
        let err = to_json(&l, Value::Table(t)).unwrap_err().to_string();
        assert!(err.contains("table key"), "the KEY is named, not just the table: {err}");
    }

    /// Nesting is where a conversion that handles each scalar correctly can still lose structure.
    #[test]
    fn nested_structures_survive_a_round_trip() {
        let l = lua();
        let original = json!({
            "name": "prova",
            "ports": [8080, 9090],
            "nested": { "deep": { "flag": true, "count": 3 } },
            "empty_obj": {},
        });
        let as_lua = json_value_to_lua(&l, &original).unwrap();
        let back = lua_value_to_json(&l, &as_lua).unwrap();
        assert_eq!(back, original, "structure survives both crossings");
    }

    /// An empty array survives re-encoding as an array. It used to come back as `{}`: decode
    /// produced a bare table, which is indistinguishable from a decoded empty OBJECT, so the shape
    /// was lost at the boundary. Found by writing these tests, 2026-08-15 — a proof that read a
    /// response, changed a field and sent it back would silently convert every empty list to an
    /// object, and plenty of APIs treat those as different requests.
    #[test]
    fn an_empty_array_survives_the_round_trip_as_an_array() {
        let l = lua();
        for original in [
            json!({ "items": [] }),
            json!({ "items": [1, 2] }),
            json!({ "items": {} }),
            json!([[], {}, [[]]]),
        ] {
            let as_lua = json_value_to_lua(&l, &original).unwrap();
            let back = lua_value_to_json(&l, &as_lua).unwrap();
            assert_eq!(back, original, "shape is preserved across the boundary");
        }
    }

    /// `sha256("abc")`, the canonical NIST vector. A digest function that is subtly wrong produces
    /// stable, plausible-looking output forever, so the only useful assertion is against a value
    /// computed somewhere other than here.
    #[test]
    fn sha256_matches_the_published_vector() {
        let l = lua();
        let hash = make_hash(&l).unwrap();
        let f: mlua::Function = hash.get("sha256").unwrap();
        let got: String = f.call("abc").unwrap();
        assert_eq!(got, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    /// base64 must be binary-safe in BOTH directions — it is what carries a non-text payload
    /// through a JSON field, so a lossy step here defeats the purpose of having it.
    #[test]
    fn base64_round_trips_bytes_that_are_not_text() {
        let l = lua();
        let b64 = make_base64(&l).unwrap();
        let enc: mlua::Function = b64.get("encode").unwrap();
        let dec: mlua::Function = b64.get("decode").unwrap();

        let raw = [0u8, 255, 128, 10, 13, 0x7f];
        let s = l.create_string(raw).unwrap();
        let encoded: String = enc.call(s).unwrap();
        let back: mlua::String = dec.call(encoded).unwrap();
        assert_eq!(back.as_bytes(), raw, "every octet survives");

        // A known vector, so the alphabet and padding are pinned too.
        let hello: String = enc.call("hello").unwrap();
        assert_eq!(hello, "aGVsbG8=");
    }

    /// `url.decode` is `url.encode`'s exact inverse, and the pair disagrees with FORM encoding on
    /// exactly one character. `+` is a literal plus here (encode emits `%20` for a space), so
    /// decoding it as a space would silently corrupt every value containing one.
    #[test]
    fn url_encoding_round_trips_and_leaves_plus_alone() {
        let l = lua();
        let url_ns = make_url(&l).unwrap();
        let enc: mlua::Function = url_ns.get("encode").unwrap();
        let dec: mlua::Function = url_ns.get("decode").unwrap();

        for original in ["a b&c=d/e", "plus+sign", "café", "100%", ""] {
            let encoded: String = enc.call(original).unwrap();
            let back: mlua::String = dec.call(encoded.clone()).unwrap();
            assert_eq!(back.to_str().unwrap().to_owned(), original, "round trip of {original:?}");
        }

        let space: String = enc.call("a b").unwrap();
        assert_eq!(space, "a%20b", "a space is %20, not +");
        let plus: mlua::String = dec.call("a+b").unwrap();
        assert_eq!(plus.to_str().unwrap().to_owned(), "a+b", "+ decodes as itself");
    }

    /// A percent sequence can carry any octet, so decode hands back BYTES. Turning an invalid one
    /// into U+FFFD would be the same silent corruption `res.body` was fixed for.
    #[test]
    fn url_decode_preserves_octets_that_are_not_utf8() {
        let l = lua();
        let url_ns = make_url(&l).unwrap();
        let dec: mlua::Function = url_ns.get("decode").unwrap();
        let back: mlua::String = dec.call("%FF%00%80").unwrap();
        assert_eq!(back.as_bytes(), [0xFF, 0x00, 0x80]);
    }

    #[test]
    fn hex_is_lowercase_and_zero_padded() {
        assert_eq!(hex_string(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(hex_string(&[]), "");
    }
}
