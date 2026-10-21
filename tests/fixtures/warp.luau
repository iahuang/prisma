-- Warp cases, picked with -p case=...
local case = param("case", "warped")

-- Content in a 0..100 square: a disc, a ring stroke and a bar.
local function content()
	return {
		Circle { cx = 35, cy = 40, r = 22, fill = "#ff0000" },
		Rect { x = 10, y = 75, w = 80, h = 12, fill = "#0000ff" },
		Circle { cx = 70, cy = 35, r = 18, fill = "none", stroke = "#00aa00", strokeWidth = 4 },
	}
end

-- The parallelogram the square goes to: x' = 200 + 3x + 1.2y, y' = 100 + 2.4y.
local M = { 3, 0, 1.2, 2.4, 200, 100 }
local function corner(x, y) return { M[1] * x + M[3] * y + M[5], M[2] * x + M[4] * y + M[6] } end
local tl, tr, br, bl = corner(0, 0), corner(100, 0), corner(100, 100), corner(0, 100)
local quad = `M{tl[1]},{tl[2]} L{tr[1]},{tr[2]} L{br[1]},{br[2]} L{bl[1]},{bl[2]} Z`
local patch = edgePatch { outline = quad, corners = { tl, tr, br, bl } }

local scene = ({
	-- strokes = "warp" so the ring's width follows the map, as a transform's does
	warped = function() return Warp { map = patch, from = { 0, 0, 100, 100 }, strokes = "warp", content() } end,
	grouped = function() return Group { transform = M, content() } end,
	edges = function()
		local p = edgePatch { top = { tl, tr }, right = { tr, br }, bottom = { bl, br }, left = { tl, bl } }
		return Warp { map = p, from = { 0, 0, 100, 100 }, strokes = "warp", content() }
	end,
	fn = function()
		return Warp { map = function(x, y) return M[1] * x + M[3] * y + M[5], M[2] * x + M[4] * y + M[6] end, strokes = "warp", content() }
	end,
	outside = function() return Warp { map = patch, from = { 0, 0, 50, 50 }, content() } end,
	no_from = function() return Warp { map = patch, content() } end,
	fn_from = function() return Warp { map = function(x, y) return x, y end, from = { 0, 0, 1, 1 }, content() } end,
	image = function() return Warp { map = patch, from = { 0, 0, 100, 100 }, Image { src = rasterize(Rect { w = 2, h = 2 }, { size = { 2, 2 } }) } } end,
	gradient = function()
		return Warp { map = patch, from = { 0, 0, 100, 100 }, Rect { w = 10, h = 10, fill = linear({ 0, 0 }, { 10, 0 }, { "#000", "#fff" }) } }
	end,
	clip = function() return Warp { map = patch, from = { 0, 0, 100, 100 }, Group { clip = { 0, 0, 5, 5 }, content() } } end,
	corners = function()
		edgePatch { outline = quad, corners = { tl, br, tr, bl } }
		return {}
	end,
	edges_apart = function()
		edgePatch { top = { tl, tr }, right = { tr, br }, bottom = { bl, br }, left = { tl, { bl[1], bl[2] + 9 } } }
		return {}
	end,
})[case]()

return Canvas { size = { 600, 400 }, bg = "#ffffff", scene }
