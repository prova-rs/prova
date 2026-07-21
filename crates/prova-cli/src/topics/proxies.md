# proxies — interposing on live traffic (mostly NOT yet shipped)

**Status first, so you don't reach for vapor:** a standalone interposing proxy — sit between
the SUT and a real dependency, observe and perturb the stream without terminating it — is a
designed direction, not a shipped surface. Do not write proofs against it.

## What IS shipped on this axis

- **Partial mocking / passthrough** — `http.mock(ctx, { target = real_url })`: unmatched
  requests pass through to the real service and are LOGGED; stubs still win. This is the
  interpose posture for the cases that matter most today: observe real traffic, override the
  interactions you're testing.
- **Record / replay** — the same object's observe dial: record real traffic once, replay it
  hermetically (the drift answer for third-party APIs).
- **In-process interposition** — `require("prova.double"){ target = real_fn }`: a proxy at a
  function-shaped seam; with no stubs it is a pure logging spy.

If the dependency speaks HTTP or is in-process, the shipped surface already covers
observe-and-override. See `prova learn doubles` for the grammar.

## What to do TODAY for the unshipped cases

| You want | Do this now |
|---|---|
| Latency/fault injection on raw TCP | run toxiproxy as a resource (`docker.run` or a plugin) and point the SUT at it |
| Observe traffic you can't route through a mock | drive the dependency's own logging/metrics, or capture at the SUT's boundary instead |
| An in-cluster shim on a container alias | not available — restructure the wiring so the SUT takes an injected URL (it should anyway) |

## The model, so the docs make sense when it ships

Three postures on one stream: a **driver** originates traffic (`prova learn drivers`), a
**double** terminates it (`prova learn doubles`), a **proxy** interposes on it. The proxy
posture will compose with the topology network vantage (aliases are where a shim would sit).
When this ships, this topic will carry the surface; until then, its absence here is the truth.
