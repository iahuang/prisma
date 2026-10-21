-- A shared helper. Its own paths (require, Image src) resolve from this file,
-- whichever script requires it.
local util = require("util")
return {
	bg = util.bg,
	swatch = function() return Image { src = "swatch.png", x = 0, y = 0, w = 10, h = 5 } end,
}
