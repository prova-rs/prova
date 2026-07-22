-- MISUSE: a flow whose body declared nothing on the builder. Zero steps is never a real suite —
-- it is the signature of ignoring the builder argument.
prova.flow("does nothing", {}, function(flow) end)
