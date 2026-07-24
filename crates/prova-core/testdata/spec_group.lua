-- Spec flags are TEST-LEVEL ONLY: a group flag would need the whole inheritance/graduation
-- ceremony back (markers on finished proofs, orphan and completion errors). Refused with the fix.
prova.group("formats", { spec = "api-freeze" }, function(g)
  g:test("open", function(t)
    t:expect(1):equals(2)
  end)
end)
