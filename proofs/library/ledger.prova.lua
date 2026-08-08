-- Prova as a LIBRARY: the obligation ledger must be reachable from outside the binary.
--
-- Prova's account of an obligation — claims, the run record, attestation, reminders — currently lives
-- in `prova-cli`. A binary is not an API, so the states a UI or an embedding host needs (CLAIMED /
-- BOUND / PROMISED / ATTESTED, and what is owed) are exactly the ones no consumer can reach.
-- prova-core says so itself, at `model.rs`'s `ReminderAccount.owed`:
--
--     "Supplied by the caller (the reconciliation lives CLI-side)."
--
-- That hole is the design being fixed. Anemnez Substrate embeds prova-core to render the ledger live
-- as a run streams; the `prova` binary and its human output are no use to it, and scraping the CLI's
-- prose would be a shortcut that ratifies the wrong architecture.
--
-- WHY THIS IS A BLACK-BOX PROOF, not a unit test: "can an outside crate reach this API?" cannot be
-- answered from inside the workspace. A unit test in prova-core sees every private item and would
-- pass while the public surface stayed empty. So this proof takes the CONSUMER's position — it writes
-- a real crate, depends on prova-core by path, and compiles it. Compilation IS the assertion.
--
-- It also pins the two decisions Phase 1 must make, in the only place that can hold them honestly:
--
--   1. PATH-INJECTED, NOT `Home`-COUPLED. `record.rs` today imports `crate::home::Home` and
--      `crate::var`, and writes `<home>/.prova/var/last-run.json`. `Home` is a project-root concept
--      (it walks up for a manifest); prova-core's `layout.rs` is a system-paths concept and does not
--      cover it. The moved API must take an explicit path so the CALLER owns path policy — an
--      embedding host resolves project roots its own way and must not be forced to adopt prova's
--      `.prova/var` convention to read a record.
--   2. NO FEATURE GATE. `serde` is `optional = true` in prova-core today, pulled in by `yaml` and the
--      cassette features. The consumer below declares `prova-core` with DEFAULT FEATURES ONLY — so if
--      the ledger sits behind an optional feature, this proof fails to compile. A ledger a consumer
--      must opt into is a ledger consumers will not find.

local REPO = prova.root

-- The consumer crate, built once for the file. `cargo build` is the whole verdict; the binary is run
-- afterwards only to check that `attest` means what it says.
local consumer = prova.fixture("ledger-consumer", Scope.File, function(ctx)
	local dir = ctx:tempdir()

	fs.write(
		dir .. "/Cargo.toml",
		table.concat({
			"[package]",
			'name = "ledger-consumer"',
			'version = "0.0.0"',
			'edition = "2021"',
			"",
			"[dependencies]",
			-- Default features only, and a path dep on THIS tree — never a published version, so the
			-- proof always grades the source under change rather than whatever crates.io holds.
			'prova-core = { path = "' .. REPO .. '/crates/prova-core" }',
			"",
			"[workspace]", -- stand alone; do not get adopted by the prova workspace above it
		}, "\n") .. "\n"
	)

	-- Names the surface Phase 1 owes. Deliberately small: the types, the reconciliation entry point,
	-- and a path-taking reader. Anything more would pin decisions this proof has no business making.
	fs.write(
		dir .. "/src/main.rs",
		table.concat({
			"use std::path::Path;",
			"",
			"use prova_core::ledger::{self, Attested, Record};",
			"",
			"fn main() {",
			"    // Path-injected: the caller says where the record is. No Home, no XDG lookup.",
			'    let path = std::env::args().nth(1).expect("usage: ledger-consumer <record.json>");',
			"    let record: Record = ledger::read_record(Path::new(&path))",
			'        .expect("a record prova itself wrote must be readable by a consumer");',
			"",
			"    // An obligation nothing claims to discharge is Unbound — the one verdict whose",
			"    // meaning cannot drift with a field rename, so it is what this proof asserts.",
			"    match ledger::attest(&record, &[]) {",
			'        Attested::Unbound => println!("UNBOUND"),',
			'        other => panic!("empty bindings must be Unbound, got {other:?}"),',
			"    }",
			"}",
		}, "\n") .. "\n"
	)

	-- The assertion. Until the ledger is library-side this fails to resolve `prova_core::ledger`, and
	-- the compiler's own words are the proof's failure message.
	-- NOT `--quiet`: the compiler's diagnostics ARE this proof's failure message, and suppressing
	-- progress output suppresses them too.
	local build = shell.run("cargo build", { cwd = dir, timeout = "900s", merge_stderr = true })
	return { dir = dir, build = build }
end)

prova.test("an outside crate can reach the obligation ledger", {
	covers = "docs/design/lifecycle.md#ledger-is-library-side",
	proves = "The RECORD half of the account is library-side: a crate outside the workspace compiles "
		.. "against prova_core::ledger with default features only. The claim half landed too (see the "
		.. "claim-ledger proof below); what remains owed is the ACCOUNT — the arithmetic over claims, "
		.. "obligations and record — which the last two proofs in this file hold open. `attest` resolves "
		.. "worst-first, so a sibling passing is not enough.",
	requires = { "cargo" },
	timeout = "900s",
}, function(t)
	local c = t:use(consumer)
	-- The console reporter renders the FIRST line of a failure message, so lead with the compiler's
	-- own first error instead of a header — a bare "…ledger:" followed by a newline teaches nothing,
	-- which is the failure mode this project calls error-without-teaching.
	-- Anchored to the rustc diagnostic shape (`error[E0433]:` / `error:`) at line start. A bare
	-- "error" substring search matches `Compiling error v2.0.19` — cargo builds a crate named
	-- `error`, so the obvious pattern reports a dependency's version as the failure.
	local first
	for line in c.build.stdout:gmatch("[^\n]+") do
		if line:match("^error%[") or line:match("^error:") then
			first = line
			break
		end
	end
	first = first or ("(no rustc diagnostic in output; cargo exit " .. tostring(c.build.code) .. ")")
	t:expect(c.build.code, "a consumer cannot build against prova-core's ledger — " .. first)
		:equals(0)
end)

prova.test("the ledger reconciles an obligation for a consumer", {
	covers = "docs/design/lifecycle.md#ledger-is-library-side",
	proves = "`attest` and a path-taking record reader are reachable from outside, and the record "
		.. "prova itself writes round-trips through them — so the reader takes paths from its caller "
		.. "rather than resolving prova's own `.prova/var` convention.",
	requires = { "cargo" },
	timeout = "900s",
}, function(t)
	local c = t:use(consumer)
	if c.build.code ~= 0 then
		t:skip("the consumer crate does not build yet — see the sibling proof")
		return
	end

	-- Read a record PROVA ITSELF wrote, rather than a hand-authored fixture: a JSON literal in this
	-- file would encode today's field names and would have to be edited by the very rename that
	-- slice 1e performs. Driving prova recursively goes through `prova.bin`, never a bare `prova`
	-- (proofs/hermeticity/binary_identity_test.lua enforces it).
	local run_dir = fs.tempdir()
	fs.write(run_dir .. "/probe.prova.lua", 'prova.test("green", function(t) t:expect(1):equals(1) end)\n')
	local rec = run_dir .. "/record.json"
	local produced = shell.run(
		prova.bin .. " --record " .. rec .. " " .. run_dir .. "/probe.prova.lua",
		{ cwd = run_dir, merge_stderr = true }
	)
	t:expect(produced.code, "the probe run failed:\n" .. produced.stdout):equals(0)
	t:expect(rec, "prova did not write the record it was asked for"):is_file()

	local out = shell.run(
		c.dir .. "/target/debug/ledger-consumer " .. rec,
		{ cwd = c.dir, merge_stderr = true }
	)
	t:expect(out.code, "the consumer could not reconcile the record:\n" .. out.stdout):equals(0)
	t:expect(out.stdout, "attest(record, []) must report Unbound"):contains("UNBOUND")
end)

-- The other half of the same account. The claim above says the ledger — claims, the record,
-- attestation, reconciliation — is computed in prova-core; only the RECORD half is there so far, and
-- `claims.rs` / `annotations.rs` are still private to prova-cli. Without this proof the claim would
-- read ATTESTED off the record half alone, which is the vacuous pass the whole lifecycle exists to
-- refuse. `attest` resolves worst-first, so while this stays open the obligation stays owed no matter
-- how green its siblings are.
local claim_consumer = prova.fixture("claim-ledger-consumer", Scope.File, function(ctx)
	local dir = ctx:tempdir()
	fs.write(
		dir .. "/Cargo.toml",
		table.concat({
			"[package]",
			'name = "claim-ledger-consumer"',
			'version = "0.0.0"',
			'edition = "2021"',
			"",
			"[dependencies]",
			'prova-core = { path = "' .. REPO .. '/crates/prova-core" }',
			"",
			"[workspace]",
		}, "\n") .. "\n"
	)
	fs.write(
		dir .. "/src/main.rs",
		table.concat({
			"use prova_core::ledger::{Claim, Owed, Status};",
			"",
			"fn main() {",
			"    // Naming the types is the assertion; the reconciliation itself is exercised by the",
			"    // record-half proofs. What is under test is whether a consumer can SEE this at all.",
			"    let _ = std::mem::size_of::<Claim>();",
			"    let _ = std::mem::size_of::<Owed>();",
			"    let _ = Status::Unproven;",
			'    println!("CLAIMS-REACHABLE");',
			"}",
		}, "\n") .. "\n"
	)
	local build = shell.run("cargo build", { cwd = dir, timeout = "900s", merge_stderr = true })
	return { dir = dir, build = build }
end)

prova.test("an outside crate can reach the claim ledger", {
	covers = "docs/design/lifecycle.md#ledger-is-library-side",
	proves = "The claim half of the account is library-side: an outside crate compiles against "
		.. "prova_core::ledger::{Claim, Status, Owed} with default features only. claims.rs moved whole "
		.. "into prova-core/src/ledger/claims.rs — it needed no redesign, having had no crate-internal "
		.. "imports and an already path-injected `scan(root, docs)`. prova-cli is now one renderer over "
		.. "that ledger rather than its owner. (The plan had named annotations.rs as part of this slice; "
		.. "that file is LuaLS/IDE wiring and has nothing to do with claim anchors — the anchor parser "
		.. "lives in claims.rs.)",
	requires = { "cargo" },
	timeout = "900s",
}, function(t)
	local c = t:use(claim_consumer)
	local first
	for line in c.build.stdout:gmatch("[^\n]+") do
		if line:match("^error%[") or line:match("^error:") then
			first = line
			break
		end
	end
	first = first or ("(no rustc diagnostic; cargo exit " .. tostring(c.build.code) .. ")")
	t:expect(c.build.code, "a consumer cannot reach the claim half of the ledger — " .. first):equals(0)
end)

-- ── Slice 1d: the ACCOUNT, not just its parts ───────────────────────────────────────────────────
--
-- Claims, obligations and the record are all library-side now, and a consumer still cannot answer
-- "where does this project stand?" — because the arithmetic that turns those three into
-- CLAIMED / BOUND / PROMISED / ATTESTED lives in prova-cli's private `evidence_account`, behind a
-- `pub(crate) struct Account`. Its own doc comment says why that placement is wrong: "ONE function,
-- because the CLI's `evidence` and the MCP `evidence` tool must be the same arithmetic or the two
-- surfaces will eventually disagree." A third surface — an embedding host — is exactly the case that
-- breaks the promise, since it cannot call a `pub(crate)` function at all and would have to
-- reimplement the counting to render a panel.
--
-- The two proofs below hold that open from both sides: the API a consumer compiles against, and the
-- wire a machine reads.

local account_consumer = prova.fixture("account-consumer", Scope.File, function(ctx)
	local dir = ctx:tempdir()

	fs.write(
		dir .. "/Cargo.toml",
		table.concat({
			"[package]",
			'name = "account-consumer"',
			'version = "0.0.0"',
			'edition = "2021"',
			"",
			"[dependencies]",
			'prova-core = { path = "' .. REPO .. '/crates/prova-core" }',
			"",
			"[workspace]",
		}, "\n") .. "\n"
	)

	-- The prose half of a tiny project, so the consumer counts something real rather than an empty
	-- vector. One anchor, nothing covering it: the intake half of the account.
	fs.write(
		dir .. "/CLAIMS.md",
		table.concat({
			"# claims",
			"",
			"<!-- claim: consumer-can-count -->",
			"The account is computed by the library, so an embedding host counts obligations itself",
			"instead of scraping the CLI's prose.",
		}, "\n") .. "\n"
	)

	fs.write(
		dir .. "/src/main.rs",
		table.concat({
			"use std::path::Path;",
			"",
			"use prova_core::ledger::{self, Account};",
			"",
			"fn main() {",
			'    let root = std::env::args().nth(1).expect("usage: account-consumer <root>");',
			"    let root = Path::new(&root);",
			"",
			"    // Path-injected, exactly as the claim half already is.",
			'    let claims = ledger::scan(root, &["CLAIMS.md".to_string()]).expect("scan the docs");',
			"",
			"    // No obligations, no record — the arithmetic must still answer. This is the shape an",
			"    // embedding host holds: three inputs it owns, one account back.",
			"    let account: Account = ledger::account(&claims, &[], None);",
			"",
			'    assert_eq!(account.bound, 0, "nothing covers this claim");',
			"    assert!(",
			"        account.attested.is_none(),",
			'        "with no record, ATTESTED is unknown — it must never render as a zero"',
			"    );",
			"",
			'    println!(',
			'        "CLAIMED={} BOUND={} PROMISED={} OWED={}",',
			"        account.claimed, account.bound, account.promised, account.owed.len()",
			"    );",
			"}",
		}, "\n") .. "\n"
	)

	local build = shell.run("cargo build", { cwd = dir, timeout = "900s", merge_stderr = true })
	return { dir = dir, build = build }
end)

prova.test("an outside crate can compute the evidence account", {
	covers = "docs/design/lifecycle.md#ledger-is-library-side",
	proves = "Slice 1d — lifted `Account` and the `evidence_account` arithmetic out of prova-cli into "
		.. "`prova_core::ledger` as `account(claims, proofs, record) -> Account`, deriving Serialize on "
		.. "it and on the `Owed`/`Status` it carries. prova-cli now resolves home/manifest/proofs/record "
		.. "and renders the account by calling the library, so an embedding host can compute the same "
		.. "account without scraping CLI prose.",
	requires = { "cargo" },
	timeout = "900s",
}, function(t)
	local c = t:use(account_consumer)

	local first
	for line in c.build.stdout:gmatch("[^\n]+") do
		if line:match("^error%[") or line:match("^error:") then
			first = line
			break
		end
	end
	first = first or ("(no rustc diagnostic; cargo exit " .. tostring(c.build.code) .. ")")
	t:expect(c.build.code, "a consumer cannot compute the account — " .. first):equals(0)

	-- Compiling is most of the assertion, but not all of it: `attested` must be an absence rather
	-- than a zero, and only running the consumer can show that. The asserts live in its `main`.
	local out = shell.run(
		c.dir .. "/target/debug/account-consumer " .. c.dir,
		{ cwd = c.dir, merge_stderr = true }
	)
	t:expect(out.code, "the consumer could not compute an account:\n" .. out.stdout):equals(0)
	t:expect(out.stdout, "the one anchored claim was not counted"):contains("CLAIMED=1")
	t:expect(out.stdout, "nothing covers it, so BOUND is 0"):contains("BOUND=0")
end)

prova.test("the account speaks machine", {
	covers = "docs/design/lifecycle.md#ledger-is-library-side",
	proves = "Slice 1d — `prova evidence --format json` now prints the serialized Account and exits 0. "
		.. "Console rendering is unchanged when `--format` is absent or `console`, and an unknown format "
		.. "is a usage error. The same library-side `Account` that an embedding host can compute can now "
		.. "be read by any machine consumer of the CLI.",
	timeout = "300s",
}, function(t)
	-- A whole tiny project, so the numbers asserted are real: one anchored claim, one proof covering
	-- it. Built in a tempdir rather than run against this repo — prova's own account moves every time
	-- someone writes a claim, and a proof that asserts against a moving number teaches nothing.
	local dir = fs.tempdir()
	fs.write(
		dir .. "/prova.toml",
		table.concat({
			"[run]",
			'proofs = ["proofs"]',
			"",
			"[specs]",
			'docs = ["CLAIMS.md"]',
		}, "\n") .. "\n"
	)
	fs.write(
		dir .. "/CLAIMS.md",
		table.concat({
			"# claims",
			"",
			"<!-- claim: machine-readable -->",
			"The account renders as data, so a client reads it without parsing prose.",
		}, "\n") .. "\n"
	)
	fs.write(
		dir .. "/proofs/covering.prova.lua",
		'prova.test("covers the claim", { covers = "CLAIMS.md#machine-readable" }, function(t)\n'
			.. "\tt:expect(1):equals(1)\n"
			.. "end)\n"
	)

	local r = shell.run(prova.bin .. " evidence --format json", { cwd = dir, merge_stderr = true })
	t:expect(r.code, "evidence --format json:\n" .. r.stdout):equals(0)

	local ok, account = pcall(json.decode, r.stdout)
	t:expect(ok and account, "the account did not decode as JSON:\n" .. r.stdout):is_truthy()
	t:expect(account.claimed, "the anchored claim was not counted"):equals(1)
	t:expect(account.bound, "the covering proof did not bind it"):equals(1)
	-- No run has been recorded in this project, so ATTESTED is unknown. JSON must say that as null
	-- rather than 0 — the same distinction the console renderer draws with an em dash.
	t:expect(account.attested, "with no recorded run, attested must be null, not 0"):is_nil()
end)
