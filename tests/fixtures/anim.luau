-- An animation whose state is a clock: s.x grows by dt each step, so frame f
-- draws a bar f / fps * 20 px wide (or, with case=dot, a 2 px dot at that x).
-- Other `-p case=...` values break one rule at a time.
local case = param("case", "ok")

return Animation {
	fps = 10,
	duration = if case == "duration" then 0.15 else 1,
	substeps = 2,
	init = function()
		if case == "state" then
			return { x = 0, tick = function() end }
		end
		return { x = 0 }
	end,
	step = function(s, dt, n)
		s.x += dt
		if case == "returns" then
			return s
		end
	end,
	frame = function(s, t, f)
		local bar = if case == "dot" then Rect { x = s.x * 20, w = 2, h = 20, fill = "#000" } else Rect { w = s.x * 20, h = 20, fill = "#000" }
		return Canvas { size = { 20, 20 }, bg = "#fff", bar }
	end,
}
