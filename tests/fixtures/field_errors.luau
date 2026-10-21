-- One misuse of an effect input per `-p case=...`; each must fail with a clear error.
local Fx = effect {
	name = "fx",
	inputs = { body = "sdf" },
	wgsl = "fn effect(p: vec2f) -> vec4f { return vec4f(body(p).cover); }",
}
local shape = Circle { cx = 50, cy = 50, r = 30 }
local cases = {
	missing = function() return Fx {} end,
	bare = function() return Fx { body = shape } end,
	list = function() return Fx { body = { Sdf { shape } } } end,
	fill = function() return Fx { body = Sdf { fill = "#f00", shape } } end,
	blur = function() return Fx { body = Sdf { blur = 4, shape } } end,
	ok = function() return Fx { body = Sdf { range = 20, shape } } end,
}
return Canvas { size = { 100, 100 }, cases[params.case]() }
