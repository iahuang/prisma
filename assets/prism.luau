-- A glass prism turning in a beam of white light: the README animation.
-- The light is traced through the prism with Snell's law, one ray per
-- wavelength, so the spectrum fans out, sweeps and folds back by total
-- internal reflection as the prism turns. A turn of 120° brings the triangle
-- back onto itself, so the loop is seamless.
--
--   prisma assets/prism.luau --sweep frame=0..59/6      a contact sheet
--   prisma assets/prism.luau -o assets/prism.gif

local W, H = 900, 400
local BG = "#0e0f13"
local CX, CY, SIDE = 330, 175, 220
local R = SIDE / math.sqrt(3) -- centroid to vertex
local BANDS = 14
local N_RED, N_VIOLET = 1.45, 1.66 -- refractive index across the spectrum (exaggerated)

local function hsl(h, s, l)
	local function f(n)
		local k = (n + h / 30) % 12
		return l - s * math.min(l, 1 - l) * math.max(-1, math.min(k - 3, 9 - k, 1))
	end
	return `rgb({f(0) * 255},{f(8) * 255},{f(4) * 255})`
end

local function triangle(theta)
	local pts = {}
	for i = 0, 2 do
		local a = math.rad(theta - 90 + 120 * i)
		pts[i + 1] = { CX + R * math.cos(a), CY + R * math.sin(a) }
	end
	return pts
end

-- Nearest edge of the triangle hit by the ray p + t d (t > 0): t and the edge's outward normal.
local function hit(tri, px, py, dx, dy)
	local best, nx, ny
	for i = 1, 3 do
		local a, b = tri[i], tri[i % 3 + 1]
		local ex, ey = b[1] - a[1], b[2] - a[2]
		local den = dx * ey - dy * ex
		if math.abs(den) > 1e-9 then
			local t = ((a[1] - px) * ey - (a[2] - py) * ex) / den
			local u = ((a[1] - px) * dy - (a[2] - py) * dx) / den
			if t > 1e-6 and u >= 0 and u <= 1 and (not best or t < best) then
				local len = math.sqrt(ex * ex + ey * ey)
				best, nx, ny = t, ey / len, -ex / len
				-- Point away from the centroid.
				if nx * (a[1] - CX) + ny * (a[2] - CY) < 0 then nx, ny = -nx, -ny end
			end
		end
	end
	return best, nx, ny
end

-- The path of one wavelength through the prism: a polyline, and how many times it bounced inside.
local function trace(tri, n, px, py, dx, dy)
	local pts, inside, bounces = { { px, py } }, false, 0
	for _ = 1, 8 do
		local t, nx, ny = hit(tri, px, py, dx, dy)
		if not t then break end
		px, py = px + dx * t, py + dy * t
		table.insert(pts, { px, py })
		if dx * nx + dy * ny > 0 then nx, ny = -nx, -ny end -- face the incoming ray
		local eta = inside and n or 1 / n
		local cosi = -(dx * nx + dy * ny)
		local k = 1 - eta * eta * (1 - cosi * cosi)
		if k < 0 then -- total internal reflection
			dx, dy = dx + 2 * cosi * nx, dy + 2 * cosi * ny
			bounces += 1
		else
			local c = eta * cosi - math.sqrt(k)
			dx, dy = eta * dx + c * nx, eta * dy + c * ny
			inside = not inside
		end
		if not inside then break end
	end
	table.insert(pts, { px + dx * 1400, py + dy * 1400 })
	return pts, bounces
end

-- A faint dot grid, so the glass has something to bend. Where light passes, the dots catch its colour.
local dots, specks = {}, {}
for x = 10, W, 20 do
	for y = 10, H, 20 do
		table.insert(dots, Circle { cx = x, cy = y, r = 1.4, fill = "#ffffff3a" })
		table.insert(specks, Circle { cx = x, cy = y, r = 1.9, fill = "#fff" })
	end
end
local ground = Rect { x = -200, y = -200, w = W + 400, h = H + 400, fill = BG }

local function with(t, extra)
	local r = table.clone(t)
	for k, v in extra do r[k] = v end
	return r
end

-- The beam rises from the lower left, aimed just above the centroid.
local BEAM = math.rad(-15)
local BDX, BDY = math.cos(BEAM), math.sin(BEAM)
local SX = -20
local SY = CY - 10 + (CX - SX) * -BDY / BDX

return Animation {
	fps = 30,
	duration = "2s",

	frame = function(_, t)
		local theta = 120 * easeInOut(t / 2, 2)
		local tri = triangle(theta)
		local shape = Path { points = tri, closed = true, fill = "#fff", stroke = "#fff", strokeWidth = 10, join = "round" }
		local glass = { body = Sdf { range = 40, shape }, thickness = 34, bevel = 26, curve = 1 }

		local paths, bounces = {}, {}
		for i = 1, BANDS do
			local k = (i - 1) / (BANDS - 1)
			paths[i], bounces[i] = trace(tri, math.lerp(N_RED, N_VIOLET, k), SX, SY, BDX, BDY)
		end

		-- The incoming beam, to where it meets the glass.
		local entry = paths[1][2]
		local light = {
			Line { x1 = SX, y1 = SY, x2 = entry[1], y2 = entry[2], stroke = "#fff", strokeWidth = 16, cap = "round", blur = 9, opacity = 0.45, blend = "screen" },
			Line { x1 = SX, y1 = SY, x2 = entry[1], y2 = entry[2], stroke = "#fff", strokeWidth = 3, cap = "round", blend = "screen" },
		}
		-- Some of it glances off the first face.
		local _, nx, ny = hit(tri, SX, SY, BDX, BDY)
		local d = BDX * nx + BDY * ny
		local rx, ry = BDX - 2 * d * nx, BDY - 2 * d * ny
		table.insert(light, Line { x1 = entry[1], y1 = entry[2], x2 = entry[1] + rx * 1400, y2 = entry[2] + ry * 1400,
			stroke = "#fff", strokeWidth = 1.2, opacity = 0.25, blend = "screen" })

		-- Direction of a path's last segment, in degrees.
		local function heading(p)
			local a, b = p[#p - 1], p[#p]
			return math.deg(math.atan2(b[2] - a[2], b[1] - a[1]))
		end

		-- The spectrum: a sheet between neighbouring rays that left by the same route, then each ray as a line.
		-- Near the critical angle a ray leaves grazing the face and neighbours split far apart: no sheet there.
		local sheet, rays, glow = {}, {}, {}
		for i = 1, BANDS do
			local k = (i - 1) / (BANDS - 1)
			local color = hsl(k * 275, 1, 0.55)
			local p = paths[i]
			local q = paths[i + 1]
			if q and #p == #q and bounces[i] == bounces[i + 1] and math.abs((heading(p) - heading(q) + 180) % 360 - 180) < 4 then
				table.insert(sheet, Path { points = { p[#p - 1], p[#p], q[#q], q[#q - 1] }, closed = true, fill = color, opacity = 0.35, blend = "screen" })
			end
			table.insert(rays, Path { points = p, stroke = color, strokeWidth = 1.6, fill = "none", opacity = 0.9, blend = "screen" })
			table.insert(glow, Path { points = p, stroke = color, strokeWidth = 30, fill = "none", blend = "screen" })
		end

		-- The dots, lit by a wide, soft copy of the light.
		local backdrop = Group { ground, Group(dots),
			Mask { shape = specks, Group { blur = 12, Group(sheet), Group(glow), Group(light) } },
		}

		return Canvas { size = { W, H }, bg = BG,
			Gloss(with(glass, { light = 40, elevation = 10, shininess = 30, color = "#bfe4ff48",
				Gloss(with(glass, { light = -120, elevation = 30, shininess = 40, color = "#ffffffa0",
					Refract(with(glass, { gap = 60, dispersion = 0.2, bands = 9, env = "#ffffff60", tint = "#dfe9ff", backdrop })),
				})),
			})),
			Path { points = tri, closed = true, fill = linear({ CX, CY - R }, { CX, CY + R / 2 }, { "#ffffff14", "#ffffff04" }) },
			Group { blend = "screen", blur = 14, Group(sheet) },
			Group { blend = "screen", Group(sheet) },
			Group { blend = "screen", Group(light) },
			Group { blend = "screen", blur = 4, opacity = 0.8, Group(rays) },
			Group { blend = "screen", Group(rays) },
			Path { points = tri, closed = true, fill = "none", stroke = "#ffffff70", strokeWidth = 1.2, join = "round" },
		}
	end,
}
