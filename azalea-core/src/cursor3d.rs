use crate::position::BlockPos;

pub struct Cursor3d {
    index: usize,

    origin: BlockPos,

    width: usize,
    height: usize,
    depth: usize,

    end: usize,
}

impl Cursor3d {
    pub fn origin(&self) -> BlockPos {
        self.origin
    }
}

impl Iterator for Cursor3d {
    type Item = CursorIteration;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.end {
            return None;
        }
        let x = self.index % self.width;
        let r = self.index / self.width;
        let y = r % self.height;
        let z = r / self.height;
        self.index += 1;

        let mut iteration_type = 0;
        if x == 0 || x == self.width - 1 {
            iteration_type += 1;
        }
        if y == 0 || y == self.height - 1 {
            iteration_type += 1;
        }
        if z == 0 || z == self.depth - 1 {
            iteration_type += 1;
        }

        Some(CursorIteration {
            // wrapping: an origin at the edge of the i32 range comes from
            // absurd (server-sent) coordinates, and mustn't panic
            pos: BlockPos {
                x: self.origin.x.wrapping_add(x as i32),
                y: self.origin.y.wrapping_add(y as i32),
                z: self.origin.z.wrapping_add(z as i32),
            },
            iteration_type: iteration_type.into(),
        })
    }
}

#[repr(u8)]
#[derive(Debug, Eq, PartialEq)]
pub enum CursorIterationType {
    Inside = 0,
    Face = 1,
    Edge = 2,
    Corner = 3,
}

pub struct CursorIteration {
    pub pos: BlockPos,
    pub iteration_type: CursorIterationType,
}

impl Cursor3d {
    /// Iterate over the blocks from `origin` to `end`, both included.
    ///
    /// An empty range (`end` below `origin` on some axis) or one of more
    /// than [`Self::MAX_VOLUME`] blocks iterates over nothing. Collision
    /// boxes come from positions and velocities the server sets, so this
    /// mustn't panic or run for minutes on absurd ones (it panicked on an
    /// empty or overflowing range before).
    pub fn new(origin: BlockPos, end: BlockPos) -> Self {
        let extent = |from: i32, to: i32| (to as i64 - from as i64 + 1).max(0) as u64;
        let (width, height, depth) = (
            extent(origin.x, end.x),
            extent(origin.y, end.y),
            extent(origin.z, end.z),
        );
        let volume = width.saturating_mul(height).saturating_mul(depth);
        if volume == 0 || volume > Self::MAX_VOLUME {
            return Self {
                index: 0,
                origin,
                width: 1,
                height: 1,
                depth: 1,
                end: 0,
            };
        }

        Self {
            index: 0,

            origin,

            width: width as usize,
            height: height as usize,
            depth: depth as usize,

            end: volume as usize,
        }
    }

    /// The most blocks a cursor iterates over: 256 x 256 x 256, far more
    /// than any vanilla collision box (one tick of movement).
    pub const MAX_VOLUME: u64 = 1 << 24;
}

impl From<u8> for CursorIterationType {
    fn from(value: u8) -> Self {
        match value {
            0 => CursorIterationType::Inside,
            1 => CursorIterationType::Face,
            2 => CursorIterationType::Edge,
            3 => CursorIterationType::Corner,
            // network-safe: Cursor3d::next counts at most 3 axes
            _ => panic!("Invalid iteration type"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_huge_ranges_iterate_over_nothing() {
        // these panicked (try_into of a negative width, i32 overflow)
        let p = |x, y, z| BlockPos::new(x, y, z);
        assert_eq!(Cursor3d::new(p(1, 0, 0), p(0, 0, 0)).count(), 0);
        assert_eq!(Cursor3d::new(p(i32::MIN, 0, 0), p(i32::MAX, 0, 0)).count(), 0);
        assert_eq!(Cursor3d::new(p(0, 0, 0), p(256, 255, 255)).count(), 0);
        assert_eq!(Cursor3d::new(p(0, 0, 0), p(255, 255, 255)).count(), 1 << 24);
        let at_edge = Cursor3d::new(p(i32::MAX - 1, 0, 0), p(i32::MAX, 0, 0))
            .map(|c| c.pos.x)
            .collect::<Vec<_>>();
        assert_eq!(at_edge, vec![i32::MAX - 1, i32::MAX]);
        assert!(BlockPos::between_closed(p(1, 0, 0), p(0, 0, 0)).is_empty());
        assert_eq!(BlockPos::between_closed(p(0, 0, 0), p(1, 1, 1)).len(), 8);
    }
}
