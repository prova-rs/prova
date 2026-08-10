use std::time::Duration;

use mlua::{Lua, LuaSerdeExt, Table, UserData, UserDataMethods, Value};


fn err(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::RuntimeError(msg.into())
}

/// A GraphQL client bound to one endpoint. `client:query(q, vars?)` returns `data` (raising on
/// `errors`); `client:execute(q, vars?)` returns `{ data, errors, status }` without raising.
struct GraphqlClient {
    url: String,
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
}

/// An owned request spec (Lua-free) so nothing borrows Lua across the await.
struct Request {
    url: String,
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
    body: Vec<u8>,
}

fn build_request(
    lua: &Lua,
    client: &GraphqlClient,
    query: String,
    variables: Option<Value>,
) -> mlua::Result<Request> {
    let mut payload = serde_json::Map::new();
    payload.insert("query".into(), serde_json::Value::String(query));
    if let Some(v) = variables {
        let vars: serde_json::Value = lua.from_value(v)?;
        if !vars.is_null() {
            payload.insert("variables".into(), vars);
        }
    }
    let body = serde_json::to_vec(&serde_json::Value::Object(payload))
        .map_err(|e| err(format!("graphql: encoding request: {e}")))?;
    Ok(Request {
        url: client.url.clone(),
        headers: client.headers.clone(),
        timeout: client.timeout,
        body,
    })
}

async fn send(req: Request) -> mlua::Result<(u16, serde_json::Value)> {
    let http = reqwest::Client::new();
    let mut r = http
        .post(&req.url)
        .header("content-type", "application/json")
        .body(req.body);
    for (k, v) in req.headers {
        r = r.header(k, v);
    }
    if let Some(t) = req.timeout {
        r = r.timeout(t);
    }
    let resp = r
        .send()
        .await
        .map_err(|e| err(format!("graphql request failed: {e}")))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| err(format!("reading graphql response: {e}")))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| err(format!("graphql response is not JSON: {e}")))?;
    Ok((status, json))
}

/// Convert a JSON value to Lua, mapping every `null` — top-level or nested — to Lua `nil`
/// (mlua otherwise uses a null-sentinel lightuserdata, which no test author expects to meet:
/// `t:expect(data.thing):is_nil()` must hold for a JSON null). Trade-off: a null INSIDE an
/// array becomes a nil hole that ends the Lua sequence there; JSON APIs under test rarely
/// return interior array nulls, and nil ergonomics win for assertions.
fn json_to_lua(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    let opts = mlua::SerializeOptions::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false);
    lua.to_value_with(v, opts)
}

/// Non-empty `errors` in the response, formatted for an error message (or `None` if clean).
fn errors_of(json: &serde_json::Value) -> Option<String> {
    match json.get("errors") {
        Some(serde_json::Value::Array(a)) if !a.is_empty() => Some(
            a.iter()
                .map(|e| {
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| e.to_string())
                })
                .collect::<Vec<_>>()
                .join("; "),
        ),
        _ => None,
    }
}

impl UserData for GraphqlClient {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // query(query, variables?) → data table; raises if the response carries GraphQL errors.
        methods.add_async_method(
            "query",
            |lua, this, (query, variables): (String, Option<Value>)| {
                let req = build_request(&lua, &this, query, variables);
                async move {
                    let (_status, json) = send(req?).await?;
                    if let Some(errors) = errors_of(&json) {
                        return Err(err(format!("graphql errors: {errors}")));
                    }
                    let data = json.get("data").cloned().unwrap_or(serde_json::Value::Null);
                    json_to_lua(&lua, &data)
                }
            },
        );

        // execute(query, variables?) → { data, errors, status }; never raises on GraphQL errors.
        methods.add_async_method(
            "execute",
            |lua, this, (query, variables): (String, Option<Value>)| {
                let req = build_request(&lua, &this, query, variables);
                async move {
                    let (status, json) = send(req?).await?;
                    let out = lua.create_table()?;
                    out.set("status", status)?;
                    out.set(
                        "data",
                        json_to_lua(
                            &lua,
                            &json.get("data").cloned().unwrap_or(serde_json::Value::Null),
                        )?,
                    )?;
                    out.set(
                        "errors",
                        json_to_lua(
                            &lua,
                            &json
                                .get("errors")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null),
                        )?,
                    )?;
                    Ok(out)
                }
            },
        );
    }
}

pub(crate) fn make(lua: &Lua) -> mlua::Result<Table> {
    let graphql = lua.create_table()?;
    // graphql.client{ url, headers?, timeout? } → a client for one GraphQL endpoint.
    graphql.set(
        "client",
        lua.create_function(|lua, opts: Table| {
            let (url, headers, timeout) =
                super::client_opts(&opts, "graphql.client", "url")?;
            lua.create_userdata(GraphqlClient {
                url,
                headers,
                timeout,
            })
        })?,
    )?;
    Ok(graphql)
}
