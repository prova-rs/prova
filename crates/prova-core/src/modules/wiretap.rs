//! The shared transport surfaces (the Phase 1 spine, docs/plans/north-star-quality.md): the §6
//! journal exposure, the proxy transcript, the shutdown pair, the `.network` vantage, and the
//! PATH-shim handle fields. A transport implements the trait for its half and registers the
//! methods once — the surface SHAPE cannot drift per transport, which makes §6 structural
//! rather than folklore.

use mlua::{Lua, RegistryKey, Table, UserData, UserDataFields, UserDataMethods, Value};

/// One mock-journal entry: what arrived, whether a stub matched, and which arm answered.
/// Transports whose journals carry exactly this store it directly (journals are test-sized);
/// richer transports (http, grpc) keep their own record type and share only the filter tail.
#[derive(Clone)]
pub(super) struct JournalRow {
    pub data: Vec<u8>,
    pub matched: bool,
    pub source: &'static str,
}

/// One proxy-transcript turn: direction-tagged bytes.
#[derive(Clone)]
pub(super) struct TranscriptRow {
    pub dir: &'static str,
    pub data: Vec<u8>,
}

/// The shutdown half: the wiretap holds a oneshot its `stop`/`close` fire, idempotently. (A
/// type with MORE to do at close — socket's proxy flushes its cassette, http's reports handler
/// errors — keeps its own methods instead of implementing this.)
pub(super) trait Shutdown {
    fn take_shutdown(&self) -> Option<tokio::sync::oneshot::Sender<()>>;
}

/// Register the `stop`/`close` pair — one action, both grammars (a proxy is a connection-shaped
/// thing that closes; a resource stops).
pub(super) fn add_shutdown_methods<T, M>(methods: &mut M)
where
    T: Shutdown + UserData + 'static,
    M: UserDataMethods<T>,
{
    methods.add_method("stop", |_, this, ()| {
        if let Some(tx) = this.take_shutdown() {
            let _ = tx.send(());
        }
        Ok(())
    });
    methods.add_method("close", |_, this, ()| {
        if let Some(tx) = this.take_shutdown() {
            let _ = tx.send(());
        }
        Ok(())
    });
}

/// The mock-journal half: a snapshot of the rows (seq is positional, 1-based at exposure).
pub(super) trait MockJournal {
    fn journal_rows(&self) -> Vec<JournalRow>;
}

/// Register `received(filter?)` — the §6 journal: `{ seq, data, matched, source }` through the
/// shared filter contract (`journal_keep`).
pub(super) fn add_received_method<T, M>(methods: &mut M)
where
    T: MockJournal + UserData + 'static,
    M: UserDataMethods<T>,
{
    methods.add_method("received", |lua, this, filter: Option<Value>| {
        let entries: Vec<Table> = this
            .journal_rows()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let t = lua.create_table()?;
                t.set("seq", i + 1)?;
                t.set("data", lua.create_string(&r.data)?)?;
                t.set("matched", r.matched)?;
                t.set("source", r.source)?;
                Ok(t)
            })
            .collect::<mlua::Result<_>>()?;
        filtered_journal(lua, entries, &filter)
    });
}

/// The proxy-transcript half: a snapshot of the direction-tagged turns.
pub(super) trait ProxyTranscript {
    fn transcript_rows(&self) -> Vec<TranscriptRow>;
}

/// Register `transcript()` — `{ seq, dir, data }` rows, unfiltered: a wiretap is raw evidence.
pub(super) fn add_transcript_method<T, M>(methods: &mut M)
where
    T: ProxyTranscript + UserData + 'static,
    M: UserDataMethods<T>,
{
    methods.add_method("transcript", |lua, this, ()| {
        let out = lua.create_table()?;
        for (i, rec) in this.transcript_rows().iter().enumerate() {
            let t = lua.create_table()?;
            t.set("seq", i + 1)?;
            t.set("dir", rec.dir)?;
            t.set("data", lua.create_string(&rec.data)?)?;
            out.set(i + 1, t)?;
        }
        Ok(out)
    });
}

/// Build `received(filter?)` entries from a snapshot of a transport's OWN record type (http and
/// grpc journals carry richer fields than [`JournalRow`]), then filter. Takes the rows by value:
/// the caller's state borrow must end before the first predicate call, because a §6 filter
/// predicate may re-enter the mock.
pub(super) fn received_from<R>(
    lua: &Lua,
    rows: Vec<R>,
    filter: &Option<Value>,
    to_lua: impl Fn(&Lua, &R, usize) -> mlua::Result<Table>,
) -> mlua::Result<Table> {
    let entries: Vec<Table> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| to_lua(lua, r, i + 1))
        .collect::<mlua::Result<_>>()?;
    filtered_journal(lua, entries, filter)
}

/// Spawn the accept loop every in-process TCP wiretap runs: accept until the shutdown oneshot
/// fires, handing each connection to the transport's own handler (which spawns its pump).
pub(super) fn spawn_accept_loop(
    listener: tokio::net::TcpListener,
    mut rx: tokio::sync::oneshot::Receiver<()>,
    on_conn: impl Fn(tokio::net::TcpStream) + 'static,
) {
    tokio::task::spawn_local(async move {
        loop {
            tokio::select! {
                _ = &mut rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else { break };
                    on_conn(stream);
                }
            }
        }
    });
}

/// Stamp the trait impls for an `Rc<RefCell<State>>`-shaped wiretap type. The bodies are
/// identical for every such transport — the macros keep the census honest about that instead of
/// letting each module hand-copy them.
macro_rules! impl_journal {
    ($ty:ty) => {
        impl crate::modules::wiretap::MockJournal for $ty {
            fn journal_rows(&self) -> Vec<crate::modules::wiretap::JournalRow> {
                self.state.borrow().journal.clone()
            }
        }
    };
}
macro_rules! impl_transcript {
    ($ty:ty) => {
        impl crate::modules::wiretap::ProxyTranscript for $ty {
            fn transcript_rows(&self) -> Vec<crate::modules::wiretap::TranscriptRow> {
                self.state.borrow().transcript.clone()
            }
        }
    };
}
macro_rules! impl_shutdown {
    ($ty:ty) => {
        impl crate::modules::wiretap::Shutdown for $ty {
            fn take_shutdown(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
                self.shutdown.borrow_mut().take()
            }
        }
    };
}
pub(crate) use {impl_journal, impl_shutdown, impl_transcript};

/// The shared tail of every `received`: keep the entries the §6 filter admits, 1-based.
/// Entries are materialized BEFORE this runs so a predicate that re-enters the mock can't hit a
/// live borrow — the caller's borrow ends before the first filter call.
pub(super) fn filtered_journal(
    lua: &Lua,
    entries: Vec<Table>,
    filter: &Option<Value>,
) -> mlua::Result<Table> {
    let out = lua.create_table()?;
    let mut n = 0;
    for entry in entries {
        if super::journal_keep(lua, filter, &entry)? {
            n += 1;
            out.set(n, entry)?;
        }
    }
    Ok(out)
}

/// The `.network` vantage every host-process mock exposes the same way: present only when
/// `network` was requested, mirroring a container resource's `.network` but addressed at the
/// host gateway rather than a DNS alias, because a mock is a host process.
pub(super) fn network_table(
    lua: &Lua,
    network_host: &Option<String>,
    port: u16,
) -> mlua::Result<Value> {
    let Some(host) = network_host else {
        return Ok(Value::Nil);
    };
    let t = lua.create_table()?;
    t.set("url", format!("http://{host}:{port}"))?;
    t.set("host", host.clone())?;
    t.set("port", port)?;
    Ok(Value::Table(t))
}

/// The PATH-shim handle half (`shell.proxy`, `terminal.proxy`): the environment to run the SUT
/// under (PATH with the shim dir prepended) and the shim's own path.
pub(super) trait ShimHandle {
    fn env_key(&self) -> &RegistryKey;
    fn shim_path(&self) -> String;
}

/// Register the `env`/`path` fields every PATH shim exposes identically.
pub(super) fn add_shim_fields<T, F>(fields: &mut F)
where
    T: ShimHandle + UserData + 'static,
    F: UserDataFields<T>,
{
    fields.add_field_method_get("env", |lua, this| lua.registry_value::<Table>(this.env_key()));
    fields.add_field_method_get("path", |_, this| Ok(this.shim_path()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(lua: &Lua) -> Vec<Table> {
        [(1, "alpha", true, "stub"), (2, "beta", false, "unmatched"), (3, "gamma", true, "stub")]
            .iter()
            .map(|(seq, data, matched, source)| {
                let t = lua.create_table().unwrap();
                t.set("seq", *seq).unwrap();
                t.set("data", *data).unwrap();
                t.set("matched", *matched).unwrap();
                t.set("source", *source).unwrap();
                t
            })
            .collect()
    }

    /// The §6 filter contract over the shared tail: nil keeps everything; a table is the same
    /// structural-subset match as `:matches` (so `{ matched = false }` answers the misses); a
    /// function is an arbitrary predicate. The output re-indexes 1..n while each entry keeps its
    /// original `seq` — position in the answer never lies about position in the journal.
    #[test]
    fn filtered_journal_speaks_the_filter_contract() {
        let lua = Lua::new();

        let all = filtered_journal(&lua, entries(&lua), &None).unwrap();
        assert_eq!(all.raw_len(), 3);

        let shape = lua.create_table().unwrap();
        shape.set("matched", false).unwrap();
        let misses = filtered_journal(&lua, entries(&lua), &Some(Value::Table(shape))).unwrap();
        assert_eq!(misses.raw_len(), 1);
        let only: Table = misses.get(1).unwrap();
        assert_eq!(only.get::<String>("data").unwrap(), "beta");
        assert_eq!(only.get::<i64>("seq").unwrap(), 2, "the journal seq survives re-indexing");

        let pred = lua
            .load(r#"function(e) return e.data == "gamma" end"#)
            .eval::<mlua::Function>()
            .unwrap();
        let picked = filtered_journal(&lua, entries(&lua), &Some(Value::Function(pred))).unwrap();
        assert_eq!(picked.raw_len(), 1);
        let only: Table = picked.get(1).unwrap();
        assert_eq!(only.get::<i64>("seq").unwrap(), 3);
    }

    /// A filter of the wrong type is a taught error, not a silent keep-nothing.
    #[test]
    fn filtered_journal_refuses_a_scalar_filter() {
        let lua = Lua::new();
        let err = filtered_journal(&lua, entries(&lua), &Some(Value::Integer(7)));
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("table") && msg.contains("function"), "teaches the contract: {msg}");
    }
}
