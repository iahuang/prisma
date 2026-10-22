-- The prelude's easing curves: endpoints, clamping, powers and the spring's overshoot.
local function near(a, b, what) assert(math.abs(a - b) < 1e-9, `{what}: {a} vs {b}`) end

for _, f in { easeIn, easeOut, easeInOut, easeSpring } do
	near(f(0), 0, "starts at 0")
	near(f(1), 1, "ends at 1")
	near(f(-2), 0, "clamped below")
	near(f(3), 1, "clamped above")
end

near(easeIn(0.5), 0.125, "cubic by default")
near(easeIn(0.5, 2), 0.25, "power 2")
near(easeIn(0.3, 1), 0.3, "power 1 is linear")
near(easeOut(0.5), 0.875, "easeOut mirrors easeIn")
near(easeOut(0.3, 4), 1 - easeIn(0.7, 4), "easeOut(k) = 1 - easeIn(1 - k)")
near(easeInOut(0.5), 0.5, "easeInOut passes the midpoint")
near(easeInOut(0.25), 0.0625, "first half is a scaled easeIn")
near(easeInOut(0.8, 2), 1 - easeInOut(0.2, 2), "easeInOut is symmetric")

local peak = 0
for i = 0, 100 do peak = math.max(peak, easeSpring(i / 100)) end
assert(peak > 1.1 and peak < 1.3, `the default spring overshoots by 10-30%: {peak}`)
local stiff = 0
for i = 0, 100 do stiff = math.max(stiff, easeSpring(i / 100, 2, 12)) end
assert(stiff < peak and stiff > 1, `damping shrinks the overshoot: {stiff}`)
near(easeSpring(1 / 7, 3), 1, "crosses 1 at k = 1 / (2 swings + 1)")

return Canvas { size = { 8, 8 } }
