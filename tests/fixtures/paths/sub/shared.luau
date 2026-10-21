-- A script in another folder using the shared helper in ../lib.
local kit = require("../lib/kit")
return Canvas { size = { 10, 10 }, bg = kit.bg, kit.swatch() }
