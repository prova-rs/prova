use mlua::{Function, Lua, Table, UserData, UserDataMethods, Value};
use sqlx::any::{AnyPoolOptions, AnyRow, AnyTypeInfoKind};
use sqlx::{AnyPool, Column, Row};

/// Which SQL engine a namespace fronts. Every engine's `client(url)` returns the same generic
/// `Connection` (sqlx `Any` driver) — the namespace exists for discoverability and URL-scheme
/// validation, not for a per-engine API.
#[derive(Clone, Copy)]
pub(crate) enum Engine {
    Sqlite,
}

impl Engine {
    fn name(self) -> &'static str {
        match self {
            Engine::Sqlite => "sqlite",
        }
    }
    fn schemes(self) -> &'static [&'static str] {
        match self {
            Engine::Sqlite => &["sqlite://", "sqlite:"],
        }
    }
}

/// A database connection pool from
/// `sqlite.client(url)`. All three return this same type. Methods are async; pair with
/// `ctx:manage(conn)` (or `ctx:defer(function() conn:close() end)`).
struct Connection {
    pool: AnyPool,
}

impl UserData for Connection {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // Run a statement (INSERT/UPDATE/DDL); returns rows affected.
        methods.add_async_method(
            "execute",
            |_, this, (sql, params): (String, Option<Vec<Value>>)| {
                let pool = this.pool.clone();
                let binds = to_binds(params);
                async move {
                    let binds = binds?;
                    let result = bound(&sql, &binds).execute(&pool).await.map_err(db_err)?;
                    Ok(result.rows_affected() as i64)
                }
            },
        );

        // Run a query; returns a list of rows, each a table of column name -> value.
        methods.add_async_method(
            "query",
            |lua, this, (sql, params): (String, Option<Vec<Value>>)| {
                let pool = this.pool.clone();
                let binds = to_binds(params);
                async move {
                    let binds = binds?;
                    let rows = bound(&sql, &binds).fetch_all(&pool).await.map_err(db_err)?;
                    let out = lua.create_table()?;
                    for (i, row) in rows.iter().enumerate() {
                        out.set(i + 1, row_to_table(&lua, row)?)?;
                    }
                    Ok(out)
                }
            },
        );

        // Query returning a single scalar (first column of the first row), or nil.
        methods.add_async_method(
            "query_value",
            |lua, this, (sql, params): (String, Option<Vec<Value>>)| {
                let pool = this.pool.clone();
                let binds = to_binds(params);
                async move {
                    let binds = binds?;
                    let row = bound(&sql, &binds)
                        .fetch_optional(&pool)
                        .await
                        .map_err(db_err)?;
                    match row {
                        Some(row) => match row.columns().first() {
                            Some(col) => {
                                extract(&lua, &row, col.ordinal(), col.type_info().kind())
                            }
                            None => Ok(Value::Nil),
                        },
                        None => Ok(Value::Nil),
                    }
                }
            },
        );

        methods.add_async_method("close", |_, this, ()| {
            let pool = this.pool.clone();
            async move {
                pool.close().await;
                Ok(())
            }
        });
    }
}

pub(crate) fn make(lua: &Lua, engine: Engine) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("client", client_fn(lua, engine)?)?;
    Ok(table)
}

fn client_fn(lua: &Lua, engine: Engine) -> mlua::Result<Function> {
    lua.create_async_function(move |lua, url: String| async move {
        let name = engine.name();
        if !engine.schemes().iter().any(|s| url.starts_with(s)) {
            return Err(mlua::Error::RuntimeError(format!(
                "{name}.client: expected a {scheme} URL, got {url:?}",
                scheme = engine.schemes()[0]
            )));
        }
        sqlx::any::install_default_drivers(); // idempotent
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .map_err(|e| mlua::Error::RuntimeError(format!("{name}.client {url:?}: {e}")))?;
        lua.create_userdata(Connection { pool })
    })
}

/// An owned bind parameter (converted off the Lua boundary so nothing borrows Lua across await).
enum Bind {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Null,
}

fn to_binds(params: Option<Vec<Value>>) -> mlua::Result<Vec<Bind>> {
    params
        .unwrap_or_default()
        .into_iter()
        .map(|v| match v {
            Value::Integer(i) => Ok(Bind::Int(i)),
            Value::Number(n) => Ok(Bind::Float(n)),
            Value::Boolean(b) => Ok(Bind::Bool(b)),
            Value::String(s) => Ok(Bind::Str(s.to_str()?.to_string())),
            Value::Nil => Ok(Bind::Null),
            other => Err(mlua::Error::RuntimeError(format!(
                "sql: unsupported parameter type {}",
                other.type_name()
            ))),
        })
        .collect()
}

fn bound<'q>(
    sql: &'q str,
    binds: &'q [Bind],
) -> sqlx::query::Query<'q, sqlx::Any, sqlx::any::AnyArguments<'q>> {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = match b {
            Bind::Int(i) => q.bind(*i),
            Bind::Float(f) => q.bind(*f),
            Bind::Bool(x) => q.bind(*x),
            Bind::Str(s) => q.bind(s.as_str()),
            Bind::Null => q.bind(Option::<String>::None),
        };
    }
    q
}

fn db_err(e: sqlx::Error) -> mlua::Error {
    mlua::Error::RuntimeError(format!("sql error: {e}"))
}

fn row_to_table(lua: &Lua, row: &AnyRow) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    for col in row.columns() {
        let value = extract(lua, row, col.ordinal(), col.type_info().kind())?;
        table.set(col.name(), value)?;
    }
    Ok(table)
}

/// Extract one column as a Lua value, mapping SQL NULL to nil. Concrete SQL types use a precise
/// decode; a column with no declared type (`Null` kind — e.g. SQLite `count(*)` and other
/// computed columns) is probed by trying candidate types in order.
fn extract(lua: &Lua, row: &AnyRow, i: usize, kind: AnyTypeInfoKind) -> mlua::Result<Value> {
    use AnyTypeInfoKind as K;
    let value = match kind {
        K::Null => return extract_untyped(lua, row, i),
        K::Bool => to_value(
            row.try_get::<Option<bool>, _>(i)
                .map_err(db_err)?
                .map(Value::Boolean),
        ),
        K::SmallInt => to_value(
            row.try_get::<Option<i16>, _>(i)
                .map_err(db_err)?
                .map(|n| Value::Integer(n as i64)),
        ),
        K::Integer => to_value(
            row.try_get::<Option<i32>, _>(i)
                .map_err(db_err)?
                .map(|n| Value::Integer(n as i64)),
        ),
        K::BigInt => to_value(
            row.try_get::<Option<i64>, _>(i)
                .map_err(db_err)?
                .map(Value::Integer),
        ),
        K::Real => to_value(
            row.try_get::<Option<f32>, _>(i)
                .map_err(db_err)?
                .map(|n| Value::Number(n as f64)),
        ),
        K::Double => to_value(
            row.try_get::<Option<f64>, _>(i)
                .map_err(db_err)?
                .map(Value::Number),
        ),
        K::Text => match row.try_get::<Option<String>, _>(i).map_err(db_err)? {
            Some(s) => Value::String(lua.create_string(&s)?),
            None => Value::Nil,
        },
        K::Blob => match row.try_get::<Option<Vec<u8>>, _>(i).map_err(db_err)? {
            Some(b) => Value::String(lua.create_string(b)?),
            None => Value::Nil,
        },
    };
    Ok(value)
}

fn to_value(opt: Option<Value>) -> Value {
    opt.unwrap_or(Value::Nil)
}

/// Probe a column of unknown declared type by trying candidate decodes in order. `Ok(None)` from
/// a decode means a real SQL NULL → nil; an `Err` means "wrong type, try the next". Integer
/// before float before bool keeps SQLite's dynamic integers integral.
fn extract_untyped(lua: &Lua, row: &AnyRow, i: usize) -> mlua::Result<Value> {
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return Ok(v.map(Value::Integer).unwrap_or(Value::Nil));
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return Ok(v.map(Value::Number).unwrap_or(Value::Nil));
    }
    if let Ok(v) = row.try_get::<Option<bool>, _>(i) {
        return Ok(v.map(Value::Boolean).unwrap_or(Value::Nil));
    }
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return match v {
            Some(s) => Ok(Value::String(lua.create_string(&s)?)),
            None => Ok(Value::Nil),
        };
    }
    if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(i) {
        return Ok(Value::String(lua.create_string(b)?));
    }
    Ok(Value::Nil)
}
