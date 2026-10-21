-- A mark built as one Path, and its construction drawn from the same geometry.
-- fillRule = "evenodd" cuts a hole wherever subpaths overlap an even number of
-- times, so a ring crossed by a bar needs no Mask. fill = "none" with a stroke
-- draws the very same path as outlines: the construction comes from the mark.
local function circle(r)
	return `M{-r},0 A{r},{r} 0 1,0 {r},0 A{r},{r} 0 1,0 {-r},0 Z`
end
local outer, inner, bar = 120, 72, 24
local mark = circle(outer) .. circle(inner) .. `M-150,{-bar} H150 V{bar} H-150 Z`

local ink, guide, accent = "#1d3557", "#9aa5b1", "#e63946"
local function label(x, y, s)
	return Text { x = x, y = y, size = 13, font = "Helvetica", fill = guide, s }
end

return Canvas { size = { 760, 360 }, bg = "#f4f1ea",
	Path { d = mark, x = 190, y = 180, fill = ink, fillRule = "evenodd" },

	Group { x = 570, y = 180,
		Line { x1 = -170, y1 = 0, x2 = 170, y2 = 0, stroke = guide, dash = { 2, 4 } },
		Line { x1 = 0, y1 = -150, x2 = 0, y2 = 150, stroke = guide, dash = { 2, 4 } },
		Path { d = mark, fill = "none", stroke = accent, strokeWidth = 1.5 },
		label(outer * 0.71 + 8, -outer * 0.71 - 6, `r {outer}`),
		label(inner * 0.71 + 8, -inner * 0.71 - 6, `r {inner}`),
		label(154, -bar - 6, `{bar * 2}`),
	},
}
