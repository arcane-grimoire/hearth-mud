use std::collections::BinaryHeap;
use std::cmp::Ordering;

use mlua::{Lua, LuaSerdeExt, UserData, UserDataMethods, Value as LuaValue};

pub struct Grid2D {
    width: usize,
    height: usize,
    cells: Vec<serde_json::Value>,
}

impl Grid2D {
    pub fn new(width: usize, height: usize, default: serde_json::Value) -> Self {
        Self {
            width,
            height,
            cells: vec![default; width * height],
        }
    }

    pub fn set_cell(&mut self, x: usize, y: usize, value: serde_json::Value) -> bool {
        if let Some(i) = self.index(x, y) {
            self.cells[i] = value;
            true
        } else {
            false
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "width": self.width,
            "height": self.height,
            "cells": self.cells,
        })
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x >= 1 && x <= self.width && y >= 1 && y <= self.height {
            Some((y - 1) * self.width + (x - 1))
        } else {
            None
        }
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        let obj = value.as_object().ok_or("grid: expected an object")?;
        let width = obj
            .get("width")
            .and_then(|v| v.as_u64())
            .ok_or("grid: missing or invalid 'width'")? as usize;
        let height = obj
            .get("height")
            .and_then(|v| v.as_u64())
            .ok_or("grid: missing or invalid 'height'")? as usize;
        let cells_arr = obj
            .get("cells")
            .and_then(|v| v.as_array())
            .ok_or("grid: missing or invalid 'cells'")?;
        let expected = width * height;
        if cells_arr.len() != expected {
            return Err(format!(
                "grid: cells length {} != width*height {}",
                cells_arr.len(),
                expected
            ));
        }
        Ok(Self {
            width,
            height,
            cells: cells_arr.clone(),
        })
    }


    fn astar(
        &self,
        start: (usize, usize),
        goal: (usize, usize),
        walkable: &serde_json::Value,
    ) -> Option<Vec<(usize, usize)>> {
        let start_i = self.index(start.0, start.1)?;
        let goal_i = self.index(goal.0, goal.1)?;

        if self.cells[start_i] != *walkable || self.cells[goal_i] != *walkable {
            return None;
        }

        let total = self.width * self.height;
        let mut g_score = vec![u32::MAX; total];
        let mut came_from = vec![usize::MAX; total];
        g_score[start_i] = 0;

        let heuristic = |i: usize| -> u32 {
            let ix = i % self.width;
            let iy = i / self.width;
            let gx = (goal.0 - 1) as i32;
            let gy = (goal.1 - 1) as i32;
            (ix as i32 - gx).unsigned_abs() + (iy as i32 - gy).unsigned_abs() 
        };

        let mut open = BinaryHeap::new();
        open.push(AStarNode {
            f_score: heuristic(start_i),
            index: start_i,
        });

        let dirs: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

        while let Some(current) = open.pop() {
            if current.index == goal_i {
                let mut path = Vec::new();
                let mut ci = goal_i;
                while ci != usize::MAX {
                    let x = ci % self.width + 1;
                    let y = ci / self.width + 1;
                    path.push((x, y));
                    ci = came_from[ci];
                }
                path.reverse();
                return Some(path);
            }

            if current.f_score > g_score[current.index].saturating_add(heuristic(current.index)) {
                continue;
            }

            let cx = (current.index % self.width) as i64;
            let cy = (current.index / self.width) as i64;

            for (dx, dy) in dirs {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || ny < 0 || nx >= self.width as i64 || ny >= self.height as i64 {
                    continue;
                }
                let ni = ny as usize * self.width + nx as usize;
                if self.cells[ni] != *walkable {
                    continue;
                }
                let tentative = g_score[current.index] + 1;
                if tentative < g_score[ni] {
                    g_score[ni] = tentative;
                    came_from[ni] = current.index;
                    open.push(AStarNode {
                        f_score: tentative + heuristic(ni),
                        index: ni,
                    });
                }
            }
        }

        None
    }

    fn has_los(
        &self,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        blocking: &serde_json::Value,
    ) -> bool {
        let mut cx = x0 as i64;
        let mut cy = y0 as i64;
        let tx = x1 as i64;
        let ty = y1 as i64;
        let dx = (tx - cx).abs();
        let dy = -(ty - cy).abs();
        let sx: i64 = if cx < tx { 1 } else { -1 };
        let sy: i64 = if cy < ty { 1 } else { -1 };
        let mut err = dx + dy;

        loop {
            if cx == tx && cy == ty {
                return true;
            }
            if let Some(i) = self.index(cx as usize, cy as usize) {
                if (cx != x0 as i64 || cy != y0 as i64) && self.cells[i] == *blocking {
                    return false;
                }
            } else {
                return false;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                cx += sx;
            }
            if e2 <= dx {
                err += dx;
                cy += sy;
            }
        }
    }

    fn fov(
        &self,
        ox: usize,
        oy: usize,
        radius: usize,
        blocking: &serde_json::Value,
    ) -> Vec<(usize, usize)> {
        let mut visible = std::collections::HashSet::new();
        visible.insert((ox, oy));

        for octant in 0..8u8 {
            self.cast_light(
                ox as i64,
                oy as i64,
                radius as i64,
                1,
                1.0,
                0.0,
                octant,
                blocking,
                &mut visible,
            );
        }

        let mut result: Vec<(usize, usize)> = visible.into_iter().collect();
        result.sort();
        result
    }

    fn cast_light(
        &self,
        ox: i64,
        oy: i64,
        radius: i64,
        row: i64,
        mut start_slope: f64,
        end_slope: f64,
        octant: u8,
        blocking: &serde_json::Value,
        visible: &mut std::collections::HashSet<(usize, usize)>,
    ) {
        if start_slope < end_slope || row > radius {
            return;
        }

        let mut blocked = false;
        let mut next_start = start_slope;

        for j in row..=radius {
            if blocked {
                break;
            }
            let dy = -j;
            for dx in -j..=0 {
                let (mx, my) = match octant {
                    0 => (ox + dx, oy + dy),
                    1 => (ox + dy, oy + dx),
                    2 => (ox - dy, oy + dx),
                    3 => (ox - dx, oy + dy),
                    4 => (ox - dx, oy - dy),
                    5 => (ox - dy, oy - dx),
                    6 => (ox + dy, oy - dx),
                    _ => (ox + dx, oy - dy),
                };

                let l_slope = (dx as f64 - 0.5) / (dy as f64 + 0.5);
                let r_slope = (dx as f64 + 0.5) / (dy as f64 - 0.5);

                if start_slope < r_slope {
                    continue;
                }
                if end_slope > l_slope {
                    break;
                }

                let dist_sq = dx * dx + dy * dy;
                if dist_sq > radius * radius {
                    continue;
                }

                if mx < 1 || my < 1 {
                    continue;
                }
                let ux = mx as usize;
                let uy = my as usize;

                if let Some(i) = self.index(ux, uy) {
                    visible.insert((ux, uy));
                    let is_blocking = self.cells[i] == *blocking;

                    if blocked {
                        if is_blocking {
                            next_start = r_slope;
                        } else {
                            blocked = false;
                            start_slope = next_start;
                        }
                    } else if is_blocking && j < radius {
                        blocked = true;
                        self.cast_light(
                            ox,
                            oy,
                            radius,
                            j + 1,
                            start_slope,
                            l_slope,
                            octant,
                            blocking,
                            visible,
                        );
                        next_start = r_slope;
                    }
                }
            }
        }
    }

    fn dijkstra_map(&self, ox: usize, oy: usize, walkable: &serde_json::Value) -> Grid2D {
        let _total = self.width * self.height;
        let max_dist = serde_json::json!(-1);
        let mut dist_grid = Grid2D::new(self.width, self.height, max_dist);

        let start_i = match self.index(ox, oy) {
            Some(i) if self.cells[i] == *walkable => i,
            _ => return dist_grid,
        };

        dist_grid.cells[start_i] = serde_json::json!(0);
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((start_i, 0u32));

        let dirs: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

        while let Some((ci, cost)) = queue.pop_front() {
            let cx = (ci % self.width) as i64;
            let cy = (ci / self.width) as i64;

            for (dx, dy) in dirs {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || ny < 0 || nx >= self.width as i64 || ny >= self.height as i64 {
                    continue;
                }
                let ni = ny as usize * self.width + nx as usize;
                if self.cells[ni] != *walkable {
                    continue;
                }
                if dist_grid.cells[ni] != serde_json::json!(-1) {
                    continue;
                }
                let new_cost = cost + 1;
                dist_grid.cells[ni] = serde_json::json!(new_cost);
                queue.push_back((ni, new_cost));
            }
        }

        dist_grid
    }

    fn flood_fill_cells(
        &mut self,
        x: usize,
        y: usize,
        target: &serde_json::Value,
        replacement: &serde_json::Value,
    ) -> u32 {
        if target == replacement {
            return 0;
        }
        let start_i = match self.index(x, y) {
            Some(i) if self.cells[i] == *target => i,
            _ => return 0,
        };

        let mut count = 0u32;
        let mut stack = vec![start_i];
        let dirs: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

        while let Some(ci) = stack.pop() {
            if self.cells[ci] != *target {
                continue;
            }
            self.cells[ci] = replacement.clone();
            count += 1;

            let cx = (ci % self.width) as i64;
            let cy = (ci / self.width) as i64;
            for (dx, dy) in dirs {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx >= 0
                    && ny >= 0
                    && (nx as usize) < self.width
                    && (ny as usize) < self.height
                {
                    let ni = ny as usize * self.width + nx as usize;
                    if self.cells[ni] == *target {
                        stack.push(ni);
                    }
                }
            }
        }

        count
    }

    pub fn install_globals(lua: &Lua) {
        let grid_new = lua
            .create_function(|lua, (w, h, default): (usize, usize, LuaValue)| {
                if w == 0 || h == 0 {
                    return Err(mlua::Error::RuntimeError(
                        "grid_new: width and height must be > 0".into(),
                    ));
                }
                let default_json: serde_json::Value = lua.from_value(default)?;
                let grid = Grid2D::new(w, h, default_json);
                lua.create_userdata(grid)
            })
            .expect("create grid_new");
        lua.globals().set("grid_new", grid_new).expect("register grid_new");

        let grid_from_value = lua
            .create_function(|lua, value: LuaValue| {
                let json: serde_json::Value = lua.from_value(value)?;
                let grid =
                    Grid2D::from_json(&json).map_err(mlua::Error::RuntimeError)?;
                lua.create_userdata(grid)
            })
            .expect("create grid_from_value");
        lua.globals()
            .set("grid_from_value", grid_from_value)
            .expect("register grid_from_value");
    }
}

#[derive(Eq, PartialEq)]
struct AStarNode {
    f_score: u32,
    index: usize,
}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f_score.cmp(&self.f_score)
    }
}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl UserData for Grid2D {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("get", |lua, this, (x, y): (usize, usize)| {
            match this.index(x, y) {
                Some(i) => lua.to_value(&this.cells[i]),
                None => Ok(LuaValue::Nil),
            }
        });

        methods.add_method_mut(
            "set",
            |lua, this, (x, y, value): (usize, usize, LuaValue)| {
                let i = this.index(x, y).ok_or_else(|| {
                    mlua::Error::RuntimeError(format!(
                        "grid:set({}, {}) out of bounds ({}x{})",
                        x, y, this.width, this.height
                    ))
                })?;
                this.cells[i] = lua.from_value(value)?;
                Ok(())
            },
        );

        methods.add_method("width", |_, this, ()| Ok(this.width));
        methods.add_method("height", |_, this, ()| Ok(this.height));
        methods.add_method("size", |_, this, ()| Ok((this.width, this.height)));

        methods.add_method_mut(
            "fill",
            |lua, this, (x1, y1, x2, y2, value): (usize, usize, usize, usize, LuaValue)| {
                let val: serde_json::Value = lua.from_value(value)?;
                let x_start = x1.max(1);
                let y_start = y1.max(1);
                let x_end = x2.min(this.width);
                let y_end = y2.min(this.height);
                for y in y_start..=y_end {
                    for x in x_start..=x_end {
                        if let Some(i) = this.index(x, y) {
                            this.cells[i] = val.clone();
                        }
                    }
                }
                Ok(())
            },
        );

        methods.add_method("to_value", |lua, this, ()| {
            lua.to_value(&this.to_json())
        });

        methods.add_method("find", |lua, this, value: LuaValue| {
            let target: serde_json::Value = lua.from_value(value)?;
            for y in 1..=this.height {
                for x in 1..=this.width {
                    let i = (y - 1) * this.width + (x - 1);
                    if this.cells[i] == target {
                        let result = lua.create_table()?;
                        result.set("x", x)?;
                        result.set("y", y)?;
                        return Ok(LuaValue::Table(result));
                    }
                }
            }
            Ok(LuaValue::Nil)
        });

        methods.add_method("find_all", |lua, this, value: LuaValue| {
            let target: serde_json::Value = lua.from_value(value)?;
            let results = lua.create_table()?;
            let mut n = 0;
            for y in 1..=this.height {
                for x in 1..=this.width {
                    let i = (y - 1) * this.width + (x - 1);
                    if this.cells[i] == target {
                        n += 1;
                        let entry = lua.create_table()?;
                        entry.set("x", x)?;
                        entry.set("y", y)?;
                        results.set(n, entry)?;
                    }
                }
            }
            Ok(LuaValue::Table(results))
        });

        methods.add_method("neighbors", |lua, this, (x, y): (usize, usize)| {
            let results = lua.create_table()?;
            let mut n = 0;
            let dirs: [(i64, i64); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
            for (dx, dy) in dirs {
                let nx = x as i64 + dx;
                let ny = y as i64 + dy;
                if nx >= 1 && ny >= 1
                    && let Some(i) = this.index(nx as usize, ny as usize) {
                        n += 1;
                        let entry = lua.create_table()?;
                        entry.set("x", nx)?;
                        entry.set("y", ny)?;
                        entry.set("value", lua.to_value(&this.cells[i])?)?;
                        results.set(n, entry)?;
                    }
            }
            Ok(LuaValue::Table(results))
        });

        methods.add_method(
            "pathfind",
            |lua, this, (x1, y1, x2, y2, walkable): (usize, usize, usize, usize, LuaValue)| {
                let walkable_json: serde_json::Value = lua.from_value(walkable)?;
                match this.astar((x1, y1), (x2, y2), &walkable_json) {
                    Some(path) => {
                        let results = lua.create_table()?;
                        for (i, (x, y)) in path.iter().enumerate() {
                            let entry = lua.create_table()?;
                            entry.set("x", *x)?;
                            entry.set("y", *y)?;
                            results.set(i + 1, entry)?;
                        }
                        Ok(LuaValue::Table(results))
                    }
                    None => Ok(LuaValue::Nil),
                }
            },
        );

        methods.add_method(
            "has_los",
            |lua, this, (x1, y1, x2, y2, blocking): (usize, usize, usize, usize, LuaValue)| {
                let blocking_json: serde_json::Value = lua.from_value(blocking)?;
                Ok(this.has_los(x1, y1, x2, y2, &blocking_json))
            },
        );

        methods.add_method(
            "fov",
            |lua, this, (x, y, radius, blocking): (usize, usize, usize, LuaValue)| {
                let blocking_json: serde_json::Value = lua.from_value(blocking)?;
                let visible = this.fov(x, y, radius, &blocking_json);
                let results = lua.create_table()?;
                for (i, (vx, vy)) in visible.iter().enumerate() {
                    let entry = lua.create_table()?;
                    entry.set("x", *vx)?;
                    entry.set("y", *vy)?;
                    results.set(i + 1, entry)?;
                }
                Ok(LuaValue::Table(results))
            },
        );

        methods.add_method(
            "distance_map",
            |lua, this, (x, y, walkable): (usize, usize, LuaValue)| {
                let walkable_json: serde_json::Value = lua.from_value(walkable)?;
                let dmap = this.dijkstra_map(x, y, &walkable_json);
                lua.create_userdata(dmap)
            },
        );

        methods.add_method_mut(
            "flood_fill",
            |lua, this, (x, y, target, replacement): (usize, usize, LuaValue, LuaValue)| {
                let target_json: serde_json::Value = lua.from_value(target)?;
                let replacement_json: serde_json::Value = lua.from_value(replacement)?;
                let count = this.flood_fill_cells(x, y, &target_json, &replacement_json);
                Ok(count)
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_grid_has_correct_dimensions() {
        let grid = Grid2D::new(5, 3, serde_json::json!(0));
        assert_eq!(grid.width, 5);
        assert_eq!(grid.height, 3);
        assert_eq!(grid.cells.len(), 15);
    }

    #[test]
    fn index_is_1_based_and_row_major() {
        let grid = Grid2D::new(3, 3, serde_json::json!(0));
        assert_eq!(grid.index(1, 1), Some(0));
        assert_eq!(grid.index(3, 1), Some(2));
        assert_eq!(grid.index(1, 2), Some(3));
        assert_eq!(grid.index(3, 3), Some(8));
    }

    #[test]
    fn index_out_of_bounds() {
        let grid = Grid2D::new(3, 3, serde_json::json!(0));
        assert_eq!(grid.index(0, 1), None);
        assert_eq!(grid.index(1, 0), None);
        assert_eq!(grid.index(4, 1), None);
        assert_eq!(grid.index(1, 4), None);
    }

    #[test]
    fn json_round_trip() {
        let mut grid = Grid2D::new(2, 2, serde_json::json!("empty"));
        grid.cells[0] = serde_json::json!("wall");
        grid.cells[3] = serde_json::json!(42);

        let json = grid.to_json();
        let restored = Grid2D::from_json(&json).unwrap();

        assert_eq!(restored.width, 2);
        assert_eq!(restored.height, 2);
        assert_eq!(restored.cells, grid.cells);
    }

    #[test]
    fn from_json_rejects_bad_input() {
        assert!(Grid2D::from_json(&serde_json::json!("not an object")).is_err());
        assert!(Grid2D::from_json(&serde_json::json!({"width": 2})).is_err());
        assert!(Grid2D::from_json(&serde_json::json!({"width": 2, "height": 2, "cells": [1]})).is_err());
    }

    #[test]
    fn astar_finds_straight_path() {
        // 5x1 corridor, all walkable
        let grid = Grid2D::new(5, 1, serde_json::json!("floor"));
        let path = grid.astar((1, 1), (5, 1), &serde_json::json!("floor"));
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 5);
        assert_eq!(path[0], (1, 1));
        assert_eq!(path[4], (5, 1));
    }

    #[test]
    fn astar_navigates_around_wall() {
        // 3x3 grid with wall in center
        let mut grid = Grid2D::new(3, 3, serde_json::json!("floor"));
        let i = grid.index(2, 2).unwrap();
        grid.cells[i] = serde_json::json!("wall");

        let path = grid.astar((1, 1), (3, 3), &serde_json::json!("floor"));
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path[0], (1, 1));
        assert_eq!(*path.last().unwrap(), (3, 3));
        assert!(!path.contains(&(2, 2)));
    }

    #[test]
    fn astar_returns_none_when_blocked() {
        // wall across the middle row blocks path
        let mut grid = Grid2D::new(3, 3, serde_json::json!("floor"));
        for x in 1..=3 {
            let i = grid.index(x, 2).unwrap();
            grid.cells[i] = serde_json::json!("wall");
        }
        let path = grid.astar((1, 1), (1, 3), &serde_json::json!("floor"));
        assert!(path.is_none());
    }

    #[test]
    fn astar_start_equals_goal() {
        let grid = Grid2D::new(3, 3, serde_json::json!("floor"));
        let path = grid.astar((2, 2), (2, 2), &serde_json::json!("floor"));
        assert!(path.is_some());
        assert_eq!(path.unwrap(), vec![(2, 2)]);
    }

    #[test]
    fn los_clear_path() {
        let grid = Grid2D::new(5, 5, serde_json::json!("floor"));
        assert!(grid.has_los(1, 1, 5, 5, &serde_json::json!("wall")));
    }

    #[test]
    fn los_blocked_by_wall() {
        let mut grid = Grid2D::new(5, 1, serde_json::json!("floor"));
        let i = grid.index(3, 1).unwrap();
        grid.cells[i] = serde_json::json!("wall");
        assert!(!grid.has_los(1, 1, 5, 1, &serde_json::json!("wall")));
    }

    #[test]
    fn fov_open_room() {
        let grid = Grid2D::new(5, 5, serde_json::json!("floor"));
        let visible = grid.fov(3, 3, 2, &serde_json::json!("wall"));
        assert!(visible.contains(&(3, 3)));
        assert!(visible.contains(&(3, 1)));
        assert!(visible.contains(&(1, 3)));
        assert!(!visible.contains(&(1, 1))); // too far (distance > 2)
    }

    #[test]
    fn fov_wall_blocks_vision() {
        let mut grid = Grid2D::new(5, 1, serde_json::json!("floor"));
        let i = grid.index(3, 1).unwrap();
        grid.cells[i] = serde_json::json!("wall");
        let visible = grid.fov(1, 1, 10, &serde_json::json!("wall"));
        assert!(visible.contains(&(1, 1)));
        assert!(visible.contains(&(2, 1)));
        assert!(visible.contains(&(3, 1))); // wall itself is visible
        assert!(!visible.contains(&(4, 1))); // behind wall
    }

    #[test]
    fn dijkstra_distances() {
        let grid = Grid2D::new(3, 1, serde_json::json!("floor"));
        let dmap = grid.dijkstra_map(1, 1, &serde_json::json!("floor"));
        assert_eq!(dmap.cells[0], serde_json::json!(0));
        assert_eq!(dmap.cells[1], serde_json::json!(1));
        assert_eq!(dmap.cells[2], serde_json::json!(2));
    }

    #[test]
    fn dijkstra_unreachable() {
        let mut grid = Grid2D::new(3, 1, serde_json::json!("floor"));
        let i = grid.index(2, 1).unwrap();
        grid.cells[i] = serde_json::json!("wall");
        let dmap = grid.dijkstra_map(1, 1, &serde_json::json!("floor"));
        assert_eq!(dmap.cells[0], serde_json::json!(0));
        assert_eq!(dmap.cells[2], serde_json::json!(-1)); // unreachable
    }

    #[test]
    fn flood_fill_region() {
        let mut grid = Grid2D::new(3, 3, serde_json::json!("floor"));
        // wall divides grid vertically
        for y in 1..=3 {
            let i = grid.index(2, y).unwrap();
            grid.cells[i] = serde_json::json!("wall");
        }
        let count = grid.flood_fill_cells(1, 1, &serde_json::json!("floor"), &serde_json::json!("water"));
        assert_eq!(count, 3); // only left column
        assert_eq!(grid.cells[grid.index(1, 1).unwrap()], serde_json::json!("water"));
        assert_eq!(grid.cells[grid.index(3, 1).unwrap()], serde_json::json!("floor")); // right side untouched
    }
}
