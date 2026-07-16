# `examples/topology/` — one definition, three verbs

A single `prova.topology("orders", …)` — a seeded Postgres wired with a Redis — consumed three ways.
The point: **your tests and your dev environment are the same description, so they cannot drift.**

Run from this directory (`cd examples/topology`); requires Docker.

## Test it

```
prova
```

Provisions the topology, runs the assertions against it, tears it down. The topology is used exactly
like a fixture (`t:use(orders)`).

## Inhabit it (attached)

```
prova up orders
```

Stands up the same topology, prints each resource's endpoint, and holds until Ctrl-C:

```
  orders — up:
    cache  redis://127.0.0.1:54981
    db     postgres://prova:prova@127.0.0.1:54982/orders

  holding — Ctrl-C to tear down
```

Now connect to the very database your tests use — `psql "postgres://prova:prova@127.0.0.1:54982/orders"`
or `redis-cli -p 54981` — and develop against it. Ctrl-C reaps everything.

## Inhabit it (detached)

```
prova start orders     # provisions in the background, prints endpoints, returns
prova ps               # list running topologies + endpoints
prova down orders      # tear it down
```

`start` spawns a held `prova up` in its own process group and returns once the topology is up; `down`
signals that holder so the same teardown runs. Same definition, same resources — just a different
terminal verb.
