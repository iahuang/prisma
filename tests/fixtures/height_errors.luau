-- One misuse of a height input per `-p case=...`; each must fail with a clear error.
local Fx = effect {
	name = "hfx",
	inputs = { surface = "height", extra = "height?" },
	wgsl = "fn effect(p: vec2f) -> vec4f { return vec4f(surface(p).h + extra(p).h); }",
}
local shape = Rect { w = 50, h = 50, fill = "#fff" }
local cases = {
	missing = function() return Fx {} end,
	bare = function() return Fx { surface = shape } end,
	sdf = function() return Fx { surface = Sdf { shape } } end,
	drawn = function() return Height { scale = 4, shape } end,
	noscale = function() return Fx { surface = Height { shape } } end,
	zero = function() return Fx { surface = Height { scale = 0, shape } } end,
	fill = function() return Fx { surface = Height { scale = 4, fill = "#fff", shape } } end,
	typo = function() return Fx { surface = Height { scale = 4, blurr = 2, shape } } end,
	ok = function() return Fx { surface = Height { scale = 4, blur = 2, shape } } end,
}
return Canvas { size = { 100, 100 }, cases[params.case]() }
