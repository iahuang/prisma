-- Scenes that outgrow vello's fixed GPU buffers unless prisma keeps layers
-- tight or splits the render. `-p case=...` picks one; each draws a red mark
-- a test looks for.
local case = param("case", "masks")
local size = param("size", 2400)

-- A small mark `depth` masks deep, in the middle of a big canvas. Each mask's
-- DestIn layer is a real blend level, and past vello's 4 in-register levels
-- every tile the layer covers spills to a buffer that holds 4096 tiles' worth.
local function nested(cx, cy)
	local mark = Rect { x = cx - 40, y = cy - 40, w = 80, h = 80, fill = "#e8402a" }
	for i = 1, param("depth", 4) do
		mark = Mask { shape = Circle { cx = cx, cy = cy, r = 60 + i * 10 }, mark }
	end
	return mark
end

-- Many large circles, each its own blended layer: tiles and layer commands
-- grow with area times count, so only splitting the render fits them.
local function crowd()
	local out = {}
	for i = 0, 1500 do
		local x, y = (i * 37) % size, (i * 53) % size
		out[#out + 1] = Circle { cx = x, cy = y, r = size / 3, fill = "#2a6ae8", opacity = 0.02, blend = "multiply" }
	end
	out[#out + 1] = Rect { x = size / 2 - 40, y = size / 2 - 40, w = 80, h = 80, fill = "#e8402a" }
	return out
end

-- More geometry than vello's line buffer holds after flattening. Splitting
-- can't help (vello flattens the whole scene), so this must fail loudly.
local function lines()
	local out = {}
	for i = 0, 25000 do
		out[#out + 1] = Circle { cx = size / 2, cy = size / 2, r = 800 + i % 100, fill = "none", stroke = "#000", strokeWidth = 0.5 }
	end
	return out
end

local cases = {
	masks = function()
		-- Plus an inverted mask (a square with a round hole), whose layers are
		-- bounded by the content rather than the shape.
		local holed = Mask { invert = true, shape = Circle { cx = 300, cy = 300, r = 50 },
			Rect { x = 200, y = 200, w = 200, h = 200, fill = "#e8402a" } }
		return { nested(size / 2, size / 2), holed }
	end,
	crowd = crowd,
	lines = lines,
}

return Canvas { size = { size, size }, bg = "#ffffff", table.unpack(cases[case]()) }
