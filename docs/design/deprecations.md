# Deprecations — prova's own removal obligations, dated

prova ships deprecation bridges so existing projects migrate rather than break — but a bridge that
never comes down is just debt. Each one is a dated **backlog item** here: work prova has said it
will do, with a deadline. The `backlog-drawdown` reminder (`proofs/reminders.prova.lua`) draws them
down — WATCHING while there is time, DUE once a date passes — so prova holds *itself* to the
schedule it asks of everyone else. This is the exemplar: the feature, demonstrated on prova.

Promote one to a claim (and delete the bridge, and prove it gone) when its time comes, or push the
date deliberately. Do not let it rot silently — that is the whole point of the date.

<!-- backlog: retire-specs-docs-shim recorded=2026-08-08 due=2027-01-01 -->
The `[specs] docs = [...]` shorthand is a deprecation bridge for the pre-`[[specs.source]]` config;
remove it, and its warning, once consumers have migrated. (`prova learn spec`.)
