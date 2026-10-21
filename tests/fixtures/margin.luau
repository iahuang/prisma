-- A blurred band that starts 300px left of the canvas and ends 20px into it.
-- Within the canvas margin, the off-canvas part still blurs in, so the left
-- edge stays solid blue; with `-p margin=0` it's gone and the edge fades.
-- `-p margin=...` is passed through as a length string (unset: the default).
local margin = param("margin", "")

return Canvas { size = { 400, 200 }, margin = margin ~= "" and margin or nil,
	Group { blur = 20, Rect { x = -300, y = -100, w = 320, h = 400, fill = "#2a6ae8" } },
}
