-- net.free_port: an OS-assigned free TCP port for a locally-spawned app that needs a dynamic port.

prova.test("free_port returns a plausible ephemeral port", function(t)
  local p = net.free_port()
  t:expect(p):gt(1024)
  t:expect(p):lte(65535)
end)

prova.test("successive calls both return valid ports", function(t)
  for _ = 1, 5 do
    local p = net.free_port()
    t:expect(p):gt(0)
  end
end)
