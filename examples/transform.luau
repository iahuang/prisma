-- Layout from measured text, placed with a general affine. `measure` returns
-- where a Text's lines and ink land, so pills fit their labels and a
-- breadcrumb chains its pieces. `transform = { a, b, c, d, e, f }` (SVG
-- matrix order) shears the whole card into a tilted "device shot" while it
-- stays vector. The red square under a plain shear is the pixel check.
local W, H = 800, 400

local function T(x, y, size, str, props)
	local t = { x = x, y = y, size = size, font = "Helvetica", fill = "#e8e8ea", str }
	for k, v in props or {} do
		t[k] = v
	end
	return Text(t)
end

-- breadcrumb: each piece starts where the previous one's advance ends
local crumbs, x = {}, 24
for i, label in { "Product", "Insights", "LIN-1305" } do
	local t = T(x, 40, 15, label, { fill = i == 3 and "#e8e8ea" or "#8a8f98" })
	table.insert(crumbs, t)
	x += measure(t).w
	if i < 3 then
		table.insert(crumbs, Path { d = `M{x + 8},31 L{x + 12},35 L{x + 8},39`, fill = "none", stroke = "#5a5e66", strokeWidth = 1.4 })
		x += 20
	end
end

-- pills sized to their labels
local pills, px = {}, 24
for _, label in { "Bug", "Performance", "Q3", "Needs design" } do
	local t = T(0, 0, 13, label, { fill = "#d6d7da" })
	local m = measure(t)
	local pad = 10
	table.insert(pills, Rect { x = px, y = 118, w = m.w + 2 * pad, h = 24, r = 12, fill = "#2a2c31" })
	t.x, t.y = px + pad, 118 + 12 + (m.ascent - m.descent) / 2
	table.insert(pills, t)
	px += m.w + 2 * pad + 8
end

local title = T(24, 90, 26, "Add source to insights slice", { weight = 700, fill = "#ffffff" })
local ink = measure(title).ink

local card = {
	Rect { x = 0, y = 0, w = 420, h = 170, r = 12, fill = "#141518", stroke = "#26282c" },
	crumbs,
	title,
	-- underline exactly as wide as the title's ink
	Line { x1 = ink.x, y1 = 100, x2 = ink.x + ink.w, y2 = 100, stroke = "#5e6ad2", strokeWidth = 2 },
	pills,
}

return Canvas { size = { W, H }, bg = "#1c1c1e",
	Group { transform = { 1.05, -0.17, 0.42, 0.92, 40, 150 }, card },
	-- x' = x + 0.5 y + 600, y' = y + 300: a 40×40 square sheared right
	Group { transform = { 1, 0, 0.5, 1, 600, 300 }, Rect { w = 40, h = 40, fill = "#ff0000" } },
}
