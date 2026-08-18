use mlua::Lua;
use noise::{NoiseFn, OpenSimplex, Perlin, Fbm, MultiFractal};

pub fn install_globals(lua: &Lua) {
    // -- Noise functions --

    // simplex2d(seed, x, y) -> number in [-1, 1]
    let f = lua
        .create_function(|_, (seed, x, y): (u32, f64, f64)| {
            let noise = OpenSimplex::new(seed);
            Ok(noise.get([x, y]))
        })
        .expect("create simplex2d");
    lua.globals().set("simplex2d", f).expect("register simplex2d");

    // simplex3d(seed, x, y, z) -> number in [-1, 1]
    let f = lua
        .create_function(|_, (seed, x, y, z): (u32, f64, f64, f64)| {
            let noise = OpenSimplex::new(seed);
            Ok(noise.get([x, y, z]))
        })
        .expect("create simplex3d");
    lua.globals().set("simplex3d", f).expect("register simplex3d");

    // perlin2d(seed, x, y) -> number in [-1, 1]
    let f = lua
        .create_function(|_, (seed, x, y): (u32, f64, f64)| {
            let noise = Perlin::new(seed);
            Ok(noise.get([x, y]))
        })
        .expect("create perlin2d");
    lua.globals().set("perlin2d", f).expect("register perlin2d");

    // perlin3d(seed, x, y, z) -> number in [-1, 1]
    let f = lua
        .create_function(|_, (seed, x, y, z): (u32, f64, f64, f64)| {
            let noise = Perlin::new(seed);
            Ok(noise.get([x, y, z]))
        })
        .expect("create perlin3d");
    lua.globals().set("perlin3d", f).expect("register perlin3d");

    // fbm2d(seed, x, y, octaves?, frequency?, lacunarity?, persistence?)
    // Fractal Brownian Motion — layered noise for natural terrain
    let f = lua
        .create_function(
            |_, (seed, x, y, octaves, frequency, lacunarity, persistence): (
                u32, f64, f64, Option<u32>, Option<f64>, Option<f64>, Option<f64>,
            )| {
                let mut fbm = Fbm::<Perlin>::new(seed);
                if let Some(o) = octaves {
                    fbm = fbm.set_octaves(o as usize);
                }
                if let Some(f) = frequency {
                    fbm = fbm.set_frequency(f);
                }
                if let Some(l) = lacunarity {
                    fbm = fbm.set_lacunarity(l);
                }
                if let Some(p) = persistence {
                    fbm = fbm.set_persistence(p);
                }
                Ok(fbm.get([x, y]))
            },
        )
        .expect("create fbm2d");
    lua.globals().set("fbm2d", f).expect("register fbm2d");

    // -- Seeded RNG --

    // hash_seed(seed, ...) -> integer
    // Deterministic hash from a seed string plus any number of integers.
    // Same inputs always produce the same output — no stored state.
    let f = lua
        .create_function(|_, args: mlua::Variadic<mlua::Value>| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for arg in args.iter() {
                match arg {
                    mlua::Value::Integer(n) => n.hash(&mut hasher),
                    mlua::Value::Number(n) => n.to_bits().hash(&mut hasher),
                    mlua::Value::String(s) => s.as_bytes().hash(&mut hasher),
                    mlua::Value::Boolean(b) => b.hash(&mut hasher),
                    other => {
                        return Err(mlua::Error::RuntimeError(format!(
                            "hash_seed: unsupported type '{}'",
                            other.type_name()
                        )));
                    }
                }
            }
            Ok(hasher.finish() as i64)
        })
        .expect("create hash_seed");
    lua.globals().set("hash_seed", f).expect("register hash_seed");

    // seed_random(seed, min, max) -> integer in [min, max]
    // Deterministic pseudo-random integer from a seed.
    let f = lua
        .create_function(|_, (seed, min, max): (i64, i64, i64)| {
            if min > max {
                return Err(mlua::Error::RuntimeError(
                    "seed_random: min must be <= max".into(),
                ));
            }
            let range = (max - min + 1) as u64;
            let value = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let result = min + (value % range) as i64;
            Ok(result)
        })
        .expect("create seed_random");
    lua.globals().set("seed_random", f).expect("register seed_random");

    // seed_float(seed) -> float in [0, 1)
    // Deterministic pseudo-random float from a seed.
    let f = lua
        .create_function(|_, seed: i64| {
            let value = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            Ok((value >> 11) as f64 / (1u64 << 53) as f64)
        })
        .expect("create seed_float");
    lua.globals().set("seed_float", f).expect("register seed_float");

    // seed_choice(seed, list) -> element from list
    // Deterministic pick from a table (1-indexed sequence).
    let f = lua
        .create_function(|_, (seed, list): (i64, mlua::Table)| {
            let len = list.raw_len();
            if len == 0 {
                return Ok(mlua::Value::Nil);
            }
            let value = (seed as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let idx = (value % len as u64) as i64 + 1;
            list.raw_get(idx)
        })
        .expect("create seed_choice");
    lua.globals().set("seed_choice", f).expect("register seed_choice");

    // -- Distance / coordinate math --

    // distance(x1, y1, x2, y2) -> Euclidean distance
    let f = lua
        .create_function(|_, (x1, y1, x2, y2): (f64, f64, f64, f64)| {
            let dx = x2 - x1;
            let dy = y2 - y1;
            Ok((dx * dx + dy * dy).sqrt())
        })
        .expect("create distance");
    lua.globals().set("distance", f).expect("register distance");

    // manhattan(x1, y1, x2, y2) -> Manhattan distance
    let f = lua
        .create_function(|_, (x1, y1, x2, y2): (f64, f64, f64, f64)| {
            Ok((x2 - x1).abs() + (y2 - y1).abs())
        })
        .expect("create manhattan");
    lua.globals().set("manhattan", f).expect("register manhattan");

    // direction_to(x1, y1, x2, y2) -> compass string (n/s/e/w/ne/nw/se/sw)
    let f = lua
        .create_function(|_, (x1, y1, x2, y2): (f64, f64, f64, f64)| {
            let dx = x2 - x1;
            let dy = y2 - y1;
            if dx == 0.0 && dy == 0.0 {
                return Ok("here");
            }
            let angle = dy.atan2(dx).to_degrees();
            let dir = match angle {
                a if (-22.5..22.5).contains(&a) => "e",
                a if (22.5..67.5).contains(&a) => "se",
                a if (67.5..112.5).contains(&a) => "s",
                a if (112.5..157.5).contains(&a) => "sw",
                a if (157.5..=180.0).contains(&a) || (-180.0..-157.5).contains(&a) => "w",
                a if (-157.5..-112.5).contains(&a) => "nw",
                a if (-112.5..-67.5).contains(&a) => "n",
                a if (-67.5..-22.5).contains(&a) => "ne",
                _ => "here",
            };
            Ok(dir)
        })
        .expect("create direction_to");
    lua.globals().set("direction_to", f).expect("register direction_to");

    // lerp(a, b, t) -> linearly interpolated value
    let f = lua
        .create_function(|_, (a, b, t): (f64, f64, f64)| {
            Ok(a + (b - a) * t)
        })
        .expect("create lerp");
    lua.globals().set("lerp", f).expect("register lerp");

    // clamp(value, min, max) -> clamped value
    let f = lua
        .create_function(|_, (value, min, max): (f64, f64, f64)| {
            Ok(value.max(min).min(max))
        })
        .expect("create clamp");
    lua.globals().set("clamp", f).expect("register clamp");

    // remap(value, in_min, in_max, out_min, out_max) -> remapped value
    let f = lua
        .create_function(|_, (value, in_min, in_max, out_min, out_max): (f64, f64, f64, f64, f64)| {
            if (in_max - in_min).abs() < f64::EPSILON {
                return Err(mlua::Error::RuntimeError("remap: input range is zero".into()));
            }
            let t = (value - in_min) / (in_max - in_min);
            Ok(out_min + (out_max - out_min) * t)
        })
        .expect("create remap");
    lua.globals().set("remap", f).expect("register remap");
}
