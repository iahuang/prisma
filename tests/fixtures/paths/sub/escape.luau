-- Reaches outside --root tests/fixtures/paths, which must fail.
require("../../sweep_effect")
return Canvas { size = { 10, 10 } }
