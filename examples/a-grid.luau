-- One glyph, rasterized once, re-drawn through a set of vector "brushes" at
-- several cell sizes. Two offset ink layers multiply into a third color.
local glyph = param("glyph", "A")
local BOX = 220

-- Blurred coverage gives a smooth 0..1 field, so brushes can taper at the edges.
local field = rasterize(
	Group { blur = 6, Text { font = "Arial Black", size = 250, x = BOX / 2, y = 235, anchor = "middle", glyph } },
	{ size = { BOX, BOX }, name = "field" }
)

local brushes = {
	dots = function(p, s, ink)
		return Circle { cx = p.x, cy = p.y, r = s / 2 * math.sqrt(p.a), fill = ink, blend = "multiply" }
	end,
	squares = function(p, s, ink)
		local k = s * p.a
		return Rect { x = p.x - k / 2, y = p.y - k / 2, w = k, h = k, fill = ink, blend = "multiply" }
	end,
	rules = function(p, s, ink)
		return Line { x1 = p.x - s / 2, y1 = p.y, x2 = p.x + s / 2, y2 = p.y, stroke = ink, strokeWidth = s * 0.8 * p.a, blend = "multiply" }
	end,
	hatch = function(p, s, ink)
		if p.a < 0.3 then return nil end
		return Line { x1 = p.x - s / 2, y1 = p.y + s / 2, x2 = p.x + s / 2, y2 = p.y - s / 2, stroke = ink, strokeWidth = s * 0.35, cap = "round", blend = "multiply" }
	end,
	rings = function(p, s, ink)
		if p.a < 0.15 then return nil end
		return Circle { cx = p.x, cy = p.y, r = s * 0.4, fill = "none", stroke = ink, strokeWidth = s * 0.3 * p.a, blend = "multiply" }
	end,
}
local names = { "dots", "squares", "rules", "hatch", "rings" }
local steps = { 7, 10, 14, 22 }

local function Cell(brush, step: number)
	local out = {}
	local pts = grid({ BOX, BOX }, step)
	local shifted = {}
	for i, p in pts do
		shifted[i] = { x = p.x - step * 0.4, y = p.y - step * 0.25 }
	end
	local a, b = sample(field, pts), sample(field, shifted)
	for i, p in a do
		if p.a > 0.02 then table.insert(out, brush(p, step, "#e8402a")) end
		local q = b[i]
		if q.a > 0.02 then table.insert(out, brush({ x = p.x, y = p.y, a = q.a }, step, "#2a5cd6")) end
	end
	return out
end

local GAP = 28
local W = #steps * (BOX + GAP) + GAP
local H = #names * (BOX + GAP + 14) + GAP + 40

local cells = {}
for row, name in names do
	for col, step in steps do
		table.insert(cells, Group { x = GAP + (col - 1) * (BOX + GAP), y = GAP + 24 + (row - 1) * (BOX + GAP + 14),
			Cell(brushes[name], step),
			Text { font = "Courier New", size = 10, y = BOX + 12, fill = "#555", `{name} / {step}` },
		})
	end
end

return Canvas { size = { W, H }, bg = "#f3efe6",
	Text { font = "Courier New", weight = 700, size = 13, x = GAP, y = GAP + 3, fill = "#222",
		`GLYPH STUDY — "{glyph}" × {#names} brushes × {#steps} cell sizes` },
	cells,
}
