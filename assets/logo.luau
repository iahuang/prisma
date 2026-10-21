-- Prisma logo: one prism, dispersed into additive R/G/B copies that recombine to white.
local W, H = 1200, 600
local INK, BG = "#f4f1ea", "#0e0f13"
local DX = param("dx", 18)    -- dispersion offset between channels
local SOLID = param("solid", false)

local function prism(cx, cy, s, fill)
	local h = s * math.sqrt(3) / 2
	return Path { points = {
		{ cx, cy - h * 2 / 3 }, { cx + s / 2, cy + h / 3 }, { cx - s / 2, cy + h / 3 },
	}, closed = true, fill = SOLID and fill or "none", stroke = fill, strokeWidth = s * (SOLID and 0.07 or 0.09), join = "round" }
end

local function mark(cx, cy, s, dx)
	return Group {
		prism(cx - dx, cy, s, "#ff2d55"),
		Group { blend = "screen", prism(cx, cy, s, "#2dff7a") },
		Group { blend = "screen", prism(cx + dx, cy, s, "#2d6bff") },
	}
end

if param("icon", false) then
	local S = 512
	return Canvas { size = { S, S }, bg = BG,
		Group { blur = 30, opacity = 0.2, mark(S / 2, S / 2 + 18, 300, DX) },
		mark(S / 2, S / 2 + 18, 300, DX),
	}
end

return Canvas { size = { W, H }, bg = BG,
	Group { blur = 40, opacity = 0.2, mark(300, 318, 300, DX) },
	mark(300, 318, 300, DX),

	Text { x = 520, y = 352, size = 170, font = "Helvetica", weight = 700, tracking = -0.045, fill = INK, "prisma" },
	Text { x = 530, y = 418, size = 24, font = "Courier New", tracking = 0.06, fill = "#8a8d98", "graphic design as code" },
}
