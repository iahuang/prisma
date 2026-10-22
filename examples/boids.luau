-- Boids: a flock simulated in `step`, drawn in `frame`.
--
--   prisma examples/boids.luau --frame 120                 one frame
--   prisma examples/boids.luau --sweep frame=0..225/45     a contact sheet over time
--   prisma examples/boids.luau -o out/boids.mp4            the whole thing (needs ffmpeg)

local W, H = 960, 540
local N = param("boids", 140)
local SUB = 2 -- steps per frame

local VIEW, SEP = 70, 22 -- neighbour and personal-space radii
local MIN_SPEED, MAX_SPEED = 70, 170
local ROCKS = { { x = 340, y = 300, r = 62 }, { x = 650, y = 220, r = 40 } }

local function hsl(h, s, l)
	local function f(n)
		local k = (n + h / 30) % 12
		return l - s * math.min(l, 1 - l) * math.max(-1, math.min(k - 3, 9 - k, 1))
	end
	return `rgb({f(0) * 255},{f(8) * 255},{f(4) * 255})`
end

-- Heading picks the colour: coral going right, teal going left, violet up or down.
local function tint(vx, vy, l)
	local k = (1 + vx / math.sqrt(vx * vx + vy * vy)) / 2
	return hsl(190 + 170 * k, 0.8, l)
end

return Animation {
	fps = 30,
	duration = "8s",
	substeps = SUB,

	init = function()
		local boids = {}
		for i = 1, N do
			local r = rng("spawn", i)
			local a = r:range(0, 2 * math.pi)
			local speed = r:range(MIN_SPEED, MAX_SPEED)
			boids[i] = { x = r:range(80, W - 80), y = r:range(80, H - 80), vx = speed * math.cos(a), vy = speed * math.sin(a) }
		end
		return { boids = boids }
	end,

	step = function(s, dt, n)
		local boids = s.boids
		-- Every boid steers from the same snapshot, so update order doesn't matter.
		local ax, ay = table.create(N, 0), table.create(N, 0)
		for i, b in boids do
			local cx, cy, avx, avy, sx, sy, count = 0, 0, 0, 0, 0, 0, 0
			for j, o in boids do
				if i ~= j then
					local dx, dy = o.x - b.x, o.y - b.y
					local d2 = dx * dx + dy * dy
					if d2 < VIEW * VIEW then
						cx, cy, avx, avy, count = cx + o.x, cy + o.y, avx + o.vx, avy + o.vy, count + 1
						if d2 < SEP * SEP and d2 > 0 then
							local d = math.sqrt(d2)
							sx, sy = sx - dx / d * (SEP - d), sy - dy / d * (SEP - d)
						end
					end
				end
			end
			local fx, fy = sx * 14, sy * 14
			if count > 0 then
				fx += (avx / count - b.vx) * 1.4 + (cx / count - b.x) * 0.7
				fy += (avy / count - b.vy) * 1.4 + (cy / count - b.y) * 0.7
			end
			-- Soft walls.
			local m = 70
			if b.x < m then fx += (m - b.x) * 8 end
			if b.x > W - m then fx -= (b.x - (W - m)) * 8 end
			if b.y < m then fy += (m - b.y) * 8 end
			if b.y > H - m then fy -= (b.y - (H - m)) * 8 end
			-- Rocks push outward within 45 px of their edge.
			for _, rock in ROCKS do
				local dx, dy = b.x - rock.x, b.y - rock.y
				local d = math.sqrt(dx * dx + dy * dy)
				local gap = d - rock.r
				if gap < 45 then
					fx += dx / d * (45 - gap) * 16
					fy += dy / d * (45 - gap) * 16
				end
			end
			-- A little wander, keyed by step and boid.
			local r = rng("wander", n, i)
			ax[i], ay[i] = fx + r:range(-60, 60), fy + r:range(-60, 60)
		end
		for i, b in boids do
			b.vx += ax[i] * dt
			b.vy += ay[i] * dt
			local speed = math.sqrt(b.vx * b.vx + b.vy * b.vy)
			local k = math.clamp(speed, MIN_SPEED, MAX_SPEED) / speed
			b.vx, b.vy = b.vx * k, b.vy * k
			b.x += b.vx * dt
			b.y += b.vy * dt
		end
	end,

	frame = function(s, t, f)
		local birds = {}
		for i, b in s.boids do
			-- A chevron seen from above; the wings sweep at each bird's own rate and phase.
			local r = rng("wings", i)
			local beat = math.abs(math.sin(2 * math.pi * (t * r:range(2.5, 3.5) + r:float())))
			local span, sweep = 3 + 5 * beat, 7 - 2 * beat
			local a = math.atan2(b.vy, b.vx)
			local c, sn = math.cos(a), math.sin(a)
			local function at(fwd, side)
				return { b.x + fwd * c - side * sn, b.y + fwd * sn + side * c }
			end
			table.insert(birds, Path { points = { at(6, 0), at(-sweep, span), at(-2, 0), at(-sweep, -span) }, closed = true, fill = tint(b.vx, b.vy, 0.72) })
		end
		local rocks = {}
		for _, r in ROCKS do
			table.insert(rocks, Circle { cx = r.x, cy = r.y, r = r.r, fill = "#1f2b40", stroke = "#33496b", strokeWidth = 2 })
		end
		return Canvas { size = { W, H }, bg = "#0b1220",
			Rect { w = W, h = H, fill = radial({ W / 2, H / 2 }, 620, { "#16233a", "#0b1220" }) },
			rocks,
			Group { blur = 4, opacity = 0.6, birds },
			birds,
			Text { x = 24, y = 34, size = 13, font = "Menlo", fill = "#5f7ea6", `boids  {N}   t = {string.format("%.2f", t)}s   frame {f}` },
		}
	end,
}
