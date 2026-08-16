--- `url.encode` and `url.decode` are inverses, and they are COMPONENT codecs
--- (docs/design/agent-ergonomics.md#url-encode-had-no-inverse).
---
--- `url.encode` shipped without a decode half, so a proof that RECEIVED a percent-encoded value —
--- a redirect's Location, a query parameter, a header — had nothing to read it with but a
--- hand-rolled decoder. Percent-decoding is precisely the "well-known place to introduce a quiet
--- bug" this module declares a crate to avoid, so the missing verb pushed authors into writing the
--- bug themselves.

prova.test("what encode writes, decode reads back", {
  covers = "docs/design/agent-ergonomics.md#url-encode-had-no-inverse",
  proves ="an encoder whose output nothing can read is half an API — and the round trip is the only assertion that catches an inverse which is subtly not one",
}, function(t)
  for _, original in ipairs({ "a b&c=d/e", "plus+sign", "café", "100%", "", "/nested/path?q=1" }) do
    t:expect(url.decode(url.encode(original)), "round trip of " .. original):equals(original)
  end
end)

prova.test("a space is %20 and a plus is a plus — the one place form encoding disagrees", {
  proves = "form encoding writes a space as `+`; component encoding writes `%20` and leaves `+` literal. The two conventions differ on exactly one character, so a decoder borrowed from the wrong one corrupts every value containing it — silently, and only for inputs that happen to include a plus",
}, function(t)
  t:expect(url.encode("a b"), "a space encodes as %20"):equals("a%20b")
  t:expect(url.decode("a+b"), "…and a plus decodes as itself"):equals("a+b")
  t:expect(url.decode("a%20b"), "…while %20 is the space"):equals("a b")
end)

prova.test("decode preserves octets that are not text", {
  proves = "a percent sequence can carry any byte, so decoding through a lossy UTF-8 conversion would replace it with U+FFFD — the same silent corruption `res.body` was fixed for, one namespace over",
}, function(t)
  local raw = url.decode("%FF%00%80")
  t:expect(#raw, "three octets in, three out"):equals(3)
  t:expect(raw:byte(1), "the first survives"):equals(255)
  t:expect(raw:byte(3), "…and the last"):equals(128)
end)
