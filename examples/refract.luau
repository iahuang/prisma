-- Glass from parts. Refract bends what's behind a body (its children) through
-- the body's surface; Gloss lights the same surface. Neither knows about any
-- material: a lens, water and bubbles are just different compositions.
--   left    a lens: a rounded slab with dispersion and a highlight
--   middle  water: a thin slab whose surface is a noise height map (Height)
--   right   bubbles: Refract with ior < 1 (air in glass), dark at their rims
--           where light reflects totally, inside a frosted glass panel
local W, H = 900, 380

-- Gray noise, for the water's height map.
local Noise = effect {
	name = "noise",
	uniforms = { size = { "length", 20 }, stretch = { "float", 1 } },
	wgsl = [[
fn effect(p: vec2f) -> vec4f {
  return vec4f(vec3f(fbm(p / (u.size * vec2f(u.stretch, 1.0)))), 1.0);
}
]],
}

local backdrop = { Rect { w = W, h = H, fill = "#f3efe6" } }
for i = 0, W // 24 do
	table.insert(backdrop, Rect { x = i * 24, y = 0, w = 12, h = H, fill = i % 2 == 0 and "#1d2a6b" or "#e8402a" })
end
table.insert(backdrop, Text { x = W / 2, y = 225, size = 120, anchor = "middle", font = "Helvetica", weight = 700, fill = "#f3efe6", "PRISMA" })

-- Props shared by a Refract and the Gloss over it.
local function with(t, extra)
	local r = table.clone(t)
	for k, v in extra do r[k] = v end
	return r
end

local lens = { body = Sdf { range = 40, Rect { x = 40, y = 70, w = 240, h = 240, r = 60 } }, thickness = 40, bevel = 40, curve = 3 }
local water = {
	body = Sdf { range = 30, Rect { x = 330, y = 70, w = 240, h = 240, r = 30 } }, thickness = 30, bevel = 30,
	surface = Height { scale = 8, blur = 1, Noise { area = { 330, 70, 240, 240 }, size = 14, stretch = 3 } },
}
local panel = Rect { x = 620, y = 70, w = 240, h = 240, r = 20 }
local bubbles = {}
for _ = 1, 40 do
	table.insert(bubbles, Circle { cx = random(640, 840), cy = random(90, 290), r = random(3, 12) })
end

return Canvas { size = { W, H },
	Gloss(with(lens, { elevation = 35, shininess = 60, color = "#ffffffc0",
		Gloss(with(water, { light = -60, elevation = 60, shininess = 300,
			Refract { body = Sdf { range = 14, bubbles }, thickness = 12, bevel = 12, ior = 0.75, env = "#1d2a6b",
				Refract { body = Sdf { range = 12, panel }, thickness = 12, bevel = 10, curve = 4, frost = 4, tint = "#e6eef0",
					Refract(with(water, { ior = 1.33, tint = "#a9d6e0", env = "#ffffff30",
						Refract(with(lens, { dispersion = 0.06, bands = 9, env = "#ffffff40",
							backdrop,
						})),
					})),
				},
			},
		})),
	})),
}
