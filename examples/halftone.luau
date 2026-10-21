-- The same source image, halftoned two ways: as a shader effect (pixels) and by
-- rasterizing it, sampling, and emitting real vector circles.
local S = 360
local cell = param("cell", 9)

-- A procedural "portrait": a head-and-shoulders silhouette filled with soft light.
local function Source()
	return Mask {
		shape = {
			Circle { cx = S / 2, cy = S * 0.4, r = S * 0.22 },
			Rect { x = S * 0.18, y = S * 0.62, w = S * 0.64, h = S * 0.5, r = S * 0.2 },
		},
		Rect { w = S, h = S, fill = linear({ 0, 0 }, { S, S }, { "#111", "#7a7a7a", "#eee" }) },
		Circle { cx = S * 0.62, cy = S * 0.3, r = S * 0.35, fill = radial({ S * 0.62, S * 0.3 }, S * 0.35, { "#fff", "#fff0" }) },
	}
end

local src = rasterize(Source(), { size = { S, S }, dpi = 48, name = "source" })
local dots = {}
for _, p in sample(src, hexGrid({ S, S }, cell)) do
	local cover = (1 - p.lum) * p.a
	if cover > 0.02 then
		table.insert(dots, Circle { cx = p.x, cy = p.y, r = (cell / 2) * 1.1 * math.sqrt(cover), fill = "#1d2a6b" })
	end
end

local labels = {}
for i, t in { "source (duotone)", "shader halftone", "rasterize → sample → vector circles" } do
	table.insert(labels, Text { font = "Courier New", size = 13, x = 20 + (i - 1) * (S + 20), y = S + 47, fill = "#333", t })
end

return Canvas { size = { S * 3 + 80, S + 90 }, bg = "#f2ead8",
	Group { x = 20, y = 20, Duotone { dark = "#1d2a6b", light = "#f2ead8", Source() } },
	Group { x = S + 40, y = 20, Halftone { cell = cell, angle = 30, ink = "#1d2a6b", Source() } },
	Group { x = S * 2 + 60, y = 20, dots },
	labels,
}
