-- Warp: bend vector content through a map. Content is drawn flat in its own
-- units; `from` says which rect of it lands on the map's unit square.
--   edgePatch { top, right, bottom, left }  a Coons patch between four curves
--   edgePatch { outline, corners }          the same, cut from one closed shape
--   along(d, { width })                     a band following a path
--   bezierPatch { 16 points }               a bicubic mesh
--   function(x, y) return x, y end          anything, over doc px

local INK, PAPER, RED = "#16130f", "#efe9da", "#e0452b"

-- A checker of beads on hairlines, in a 24 x 6 grid.
local function beads(cols, rows)
	local out = {}
	for j = 0, rows - 1 do
		table.insert(out, Line { x1 = 0, y1 = j + 0.5, x2 = cols, y2 = j + 0.5, stroke = INK, strokeWidth = 0.8 })
		for i = 0, cols - 1 do
			if (i + j) % 2 == 0 then
				table.insert(out, Ellipse { cx = i + 0.5, cy = j + 0.5, rx = 0.5, ry = 0.38, fill = INK })
			end
		end
	end
	return out
end

-- 1. A ribbon: a Coons patch between two wavy edges.
local ribbon = edgePatch {
	top = "M60,120 C220,20 380,220 560,90",
	right = "M560,90 C585,150 575,190 590,240",
	bottom = "M80,300 C260,200 400,380 590,240",
	left = "M60,120 C50,190 90,240 80,300",
}

-- 2. Type around a circle: u runs along the path, v across a 44 px band.
local R = 130
local circle = `M{900 - R},210 A{R},{R} 0 1,1 {900 + R},210 A{R},{R} 0 1,1 {900 - R},210`
-- `from` is the line's measured width, so it wraps the circle exactly once.
local ring = Text { x = 0, y = 34, size = 34, font = "Helvetica", weight = 700, fill = INK, "BEND ANY VECTOR · WARP · " }
local ringW = measure(ring).w

-- 3. A flag: a bicubic mesh; strokes = "warp" makes stroke widths follow it.
local flag = bezierPatch {
	{ 120, 440 }, { 230, 400 }, { 330, 480 }, { 460, 430 },
	{ 120, 520 }, { 230, 480 }, { 330, 560 }, { 460, 510 },
	{ 120, 600 }, { 230, 560 }, { 330, 640 }, { 460, 590 },
	{ 120, 680 }, { 230, 640 }, { 330, 720 }, { 460, 670 },
}
local stripes = {}
for k = 0, 6 do
	table.insert(stripes, Rect { x = 0, y = k * 100 / 7, w = 100, h = 100 / 14, fill = INK })
end

-- 4. A function: a sine wave over doc px.
local function wave(x, y) return x, y + 16 * math.sin((x - 600) / 46) end

return Canvas { size = { 1200, 800 }, bg = PAPER,
	Warp { map = ribbon, from = { 0, 0, 24, 6 }, beads(24, 6) },
	Path { d = "M60,120 C220,20 380,220 560,90", fill = "none", stroke = RED, strokeWidth = 2 },

	Circle { cx = 900, cy = 210, r = R - 30, fill = "none", stroke = INK, strokeWidth = 1 },
	Warp { map = along(circle, { width = 44 }), from = { 0, 0, ringW, 44 }, ring },

	Warp { map = flag, from = { 0, 0, 100, 100 }, strokes = "warp",
		stripes,
		Rect { x = 0, y = 0, w = 44, h = 50, fill = RED },
		Circle { cx = 22, cy = 25, r = 12, fill = "none", stroke = PAPER, strokeWidth = 3 },
		Rect { x = 0, y = 0, w = 100, h = 100, fill = "none", stroke = INK, strokeWidth = 2 },
	},
	Line { x1 = 120, y1 = 440, x2 = 120, y2 = 780, stroke = INK, strokeWidth = 4 },

	Warp { map = wave,
		Text { x = 600, y = 600, size = 120, font = "Helvetica", weight = 700, fill = INK, "wave" },
		Rect { x = 600, y = 640, w = 520, h = 10, fill = RED },
		Rect { x = 600, y = 662, w = 520, h = 4, fill = INK },
	},
}
